//! Measurements published on squig.link sites, found by headphone name.
//!
//! The sites have no API, but each publishes the same way: a catalogue at
//! `<site>/data/phone_book.json`, brands each with their models, and each
//! measurement as REW text at `<site>/data/<file> L.txt` and ` R.txt`. A
//! catalogue is kept beside the config for a day; a site that does not answer,
//! or answers with something that is not a catalogue, is left out of the
//! search rather than guessed at. Measurements are the reviewers': fetched
//! when the person asks for one, for their own use, and credited.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use parking_lot::Mutex;

use super::targets::Ear;
use crate::config;

/// A squig.link site.
#[derive(Debug, PartialEq, Eq)]
pub struct Site {
    /// Where its `data/` folder is.
    pub base: &'static str,
    /// The rig, where the site says which.
    pub rig: Option<&'static str>,
    /// In-ear or over-ear, where the site keeps one kind.
    pub ear: Option<Ear>,
}

/// The sites searched: each answers with a catalogue in the shared layout.
pub const SITES: &[Site] = &[
    at("https://squig.link", None, None),
    at("https://squig.link/headphones", None, Some(Ear::Over)),
    at(
        "https://graph.hangout.audio/iem/711",
        Some("711"),
        Some(Ear::In),
    ),
    at(
        "https://graph.hangout.audio/iem/5128",
        Some("5128"),
        Some(Ear::In),
    ),
    at(
        "https://graph.hangout.audio/headphones",
        None,
        Some(Ear::Over),
    ),
    at("https://precog.squig.link", None, None),
    at("https://timmyv.squig.link", None, None),
    at("https://hbb.squig.link", None, None),
    at("https://kr0mka.squig.link", None, None),
    at("https://tgx78.squig.link", None, None),
    at("https://bakkwatan.squig.link", None, None),
    at("https://jaytiss.squig.link", None, None),
    at("https://doltonius.squig.link", None, None),
    at("https://listener.squig.link", None, None),
    at("https://pw.squig.link", None, None),
    at("https://kazi.squig.link", None, None),
    at("https://achoreviews.squig.link", None, None),
];

const fn at(base: &'static str, rig: Option<&'static str>, ear: Option<Ear>) -> Site {
    Site { base, rig, ear }
}

/// The site at `base`, where it is one searched.
pub fn site(base: &str) -> Option<&'static Site> {
    SITES.iter().find(|s| s.base == base)
}

impl Site {
    /// The site as a person reads it: its host and path, without the scheme.
    pub fn label(&self) -> &'static str {
        self.base.trim_start_matches("https://")
    }
}

/// A measurement in a site's catalogue.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub site: &'static Site,
    pub brand: String,
    pub model: String,
    /// How this measurement differs from the model's others: tips, inserts,
    /// a nozzle. Empty for the model's own.
    pub variant: String,
    /// The file name its two channels are kept under.
    pub file: String,
}

impl Hit {
    pub fn name(&self) -> String {
        let name = format!("{} {}", self.brand, self.model);
        if self.variant.is_empty() {
            name
        } else {
            format!("{name} {}", self.variant)
        }
    }

    /// Whom to credit: the site it was measured for, as a profile's source.
    pub fn source(&self) -> String {
        let site = self.site.label();
        if site.contains("squig.link") {
            format!("{} on {site}", self.name())
        } else {
            format!("{} on {site} (squig.link)", self.name())
        }
    }
}

/// A catalogue entry: brand, model, variant and file.
type Entry = (String, String, String, String);

const FRESH_FOR: Duration = Duration::from_secs(24 * 60 * 60);
/// The largest catalogue accepted. The largest is about 700 KB.
const BOOK_CAP: u64 = 4 << 20;
/// The largest measurement accepted. They are about 15 KB.
const MEASUREMENT_CAP: u64 = 512 << 10;

/// Each site's catalogue as last read, and when.
static READ: Mutex<Option<HashMap<&'static str, (SystemTime, Arc<Vec<Entry>>)>>> = Mutex::new(None);

/// Every site's measurements whose name has each word of `query`, best
/// first: the model named exactly, then the shortest names. A site that
/// cannot be read is left out; an error only where none can be.
pub fn search(query: &str, limit: usize) -> Result<Vec<Hit>, String> {
    let words = words(query);
    if words.is_empty() {
        return Ok(Vec::new());
    }
    let books: Vec<_> = std::thread::scope(|s| {
        let reads: Vec<_> = SITES
            .iter()
            .map(|site| s.spawn(move || (site, book(site))))
            .collect();
        reads.into_iter().filter_map(|r| r.join().ok()).collect()
    });
    if books.iter().all(|(_, b)| b.is_err()) {
        return Err("No squig.link site could be reached".into());
    }
    let mut hits: Vec<(usize, Hit)> = Vec::new();
    for (site, book) in books {
        let Ok(book) = book else { continue };
        for (brand, model, variant, file) in book.iter() {
            let text = words_of(&format!("{brand} {model} {variant}"));
            if !words
                .iter()
                .all(|w| text.iter().any(|t| t.contains(w.as_str())))
            {
                continue;
            }
            let exact = words_of(&format!("{brand} {model}")) == words || words_of(model) == words;
            let rank = if exact { 0 } else { 1_000 } + text.len() * 10 + variant.len();
            hits.push((
                rank,
                Hit {
                    site,
                    brand: brand.clone(),
                    model: model.clone(),
                    variant: variant.clone(),
                    file: file.clone(),
                },
            ));
        }
    }
    hits.sort_by_key(|(rank, h)| (*rank, h.site.base));
    Ok(hits.into_iter().take(limit).map(|(_, h)| h).collect())
}

/// The measurement `hit` names, its two channels averaged, as frequency and
/// level text: what `profiles::read_measurement` takes. One channel where
/// the site has only one.
pub fn fetch(hit: &Hit) -> Result<String, String> {
    let channel = |side: &str| -> Result<super::targets::Curve, String> {
        let url = data_url(hit.site, &format!("{} {side}.txt", hit.file))?;
        let resp = http()?
            .get(url)
            .send()
            .map_err(|e| e.without_url().to_string())?;
        if !resp.status().is_success() {
            return Err(format!("{} answered {}", hit.site.label(), resp.status()));
        }
        let curve = super::targets::points(&body(resp, MEASUREMENT_CAP)?);
        if curve.len() < 20 {
            return Err(format!(
                "{} has no measurement for {}",
                hit.site.label(),
                hit.name()
            ));
        }
        Ok(curve)
    };
    let (left, right) = (channel("L"), channel("R"));
    let curve = match (left, right) {
        (Ok(l), Ok(r)) => l
            .iter()
            .map(|&(hz, db)| (hz, (db + super::targets::at(&r, hz)) / 2.0))
            .collect(),
        (Ok(one), Err(_)) | (Err(_), Ok(one)) => one,
        (Err(e), Err(_)) => return Err(e),
    };
    let mut text = String::from("frequency,raw\n");
    for (hz, db) in curve {
        text.push_str(&format!("{hz:.3},{db:.3}\n"));
    }
    Ok(text)
}

/// `name` in `site`'s data folder, as a URL.
fn data_url(site: &Site, name: &str) -> Result<url::Url, String> {
    let mut url = url::Url::parse(&format!("{}/data/", site.base)).map_err(|e| e.to_string())?;
    url.path_segments_mut()
        .map_err(|()| format!("{} is not a site", site.base))?
        .pop_if_empty()
        .push(name);
    Ok(url)
}

/// `site`'s catalogue: the copy read this last day, the one kept beside the
/// config while it is a day old, or fetched. A copy kept, however old, is
/// used when the site does not answer.
fn book(site: &'static Site) -> Result<Arc<Vec<Entry>>, String> {
    if let Some((at, entries)) = READ.lock().as_ref().and_then(|m| m.get(site.base)) {
        if SystemTime::now()
            .duration_since(*at)
            .is_ok_and(|age| age < FRESH_FOR)
        {
            return Ok(entries.clone());
        }
    }
    let file = cache_dir().join(format!("{}.json", slug(site.base)));
    let kept = std::fs::read_to_string(&file).ok();
    let fresh = std::fs::metadata(&file)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age < FRESH_FOR);
    let entries = match kept.as_deref().map(parse_book) {
        Some(Ok(entries)) if fresh => entries,
        kept_entries => match fetch_book(site) {
            Ok((text, entries)) => {
                let part = file.with_extension("json.part");
                let written = std::fs::create_dir_all(cache_dir())
                    .and_then(|()| std::fs::write(&part, &text))
                    .and_then(|()| std::fs::rename(&part, &file));
                if let Err(e) = written {
                    log::info!("squig: {} not kept: {e}", site.label());
                    let _ = std::fs::remove_file(&part);
                }
                entries
            }
            Err(e) => match kept_entries {
                Some(Ok(entries)) => {
                    log::info!(
                        "squig: {} not refreshed, using the copy kept: {e}",
                        site.label()
                    );
                    entries
                }
                _ => {
                    log::info!("squig: {} left out: {e}", site.label());
                    return Err(e);
                }
            },
        },
    };
    let entries = Arc::new(entries);
    READ.lock()
        .get_or_insert_with(HashMap::new)
        .insert(site.base, (SystemTime::now(), entries.clone()));
    Ok(entries)
}

fn fetch_book(site: &Site) -> Result<(String, Vec<Entry>), String> {
    let resp = http()?
        .get(data_url(site, "phone_book.json")?)
        .send()
        .map_err(|e| e.without_url().to_string())?;
    if !resp.status().is_success() {
        return Err(format!("{} answered {}", site.label(), resp.status()));
    }
    let text = body(resp, BOOK_CAP)?;
    let entries = parse_book(&text)?;
    Ok((text, entries))
}

/// A field that is one string or several.
fn strings(v: Option<&serde_json::Value>) -> Vec<&str> {
    use serde_json::Value;
    match v {
        Some(Value::String(s)) => vec![s.as_str()],
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    }
}

/// A catalogue: brands, each with its models. A model is a name, or an
/// object with a `name` and a `file`, which may be several files, one per
/// variant, with a `suffix` for each saying how it differs.
fn parse_book(text: &str) -> Result<Vec<Entry>, String> {
    use serde_json::Value;
    let brands: Vec<Value> =
        serde_json::from_str(text).map_err(|_| "not a squig.link catalogue".to_owned())?;
    let mut out = Vec::new();
    for b in &brands {
        let Some(brand) = b.get("name").and_then(Value::as_str) else {
            continue;
        };
        // "_EQ" and the like are the site's own tools, not headphones.
        if brand.starts_with('_') {
            continue;
        }
        for phone in b
            .get("phones")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let (model, files, suffixes) = match phone {
                Value::String(name) => (name.as_str(), vec![name.as_str()], Vec::new()),
                Value::Object(o) => {
                    let Some(model) = o.get("name").and_then(Value::as_str) else {
                        continue;
                    };
                    let files = strings(o.get("file"));
                    let files = if files.is_empty() { vec![model] } else { files };
                    (model, files, strings(o.get("suffix")))
                }
                _ => continue,
            };
            for (i, file) in files.iter().enumerate() {
                // A file name that would leave the data folder is skipped.
                if file.contains(['/', '\\']) || file.starts_with('.') {
                    continue;
                }
                let variant = suffixes
                    .get(i)
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| {
                        // Without suffixes, a variant is what its file adds.
                        let extra = file.strip_prefix(&format!("{brand} {model}")).unwrap_or("");
                        if i == 0 {
                            String::new()
                        } else {
                            extra.trim().to_owned()
                        }
                    });
                out.push((
                    brand.to_owned(),
                    model.to_owned(),
                    variant,
                    (*file).to_owned(),
                ));
            }
        }
    }
    if out.is_empty() {
        return Err("the catalogue lists nothing".into());
    }
    Ok(out)
}

fn words(text: &str) -> Vec<String> {
    words_of(text)
}

/// Lowercase words, letters and digits only: "Performer 8S" is
/// ["performer", "8s"].
fn words_of(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn slug(base: &str) -> String {
    base.trim_start_matches("https://")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn cache_dir() -> PathBuf {
    config::config_dir().join("squig")
}

fn http() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent(concat!("koan/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())
}

/// A response's body as text, refused rather than cut short past `cap`.
fn body(resp: reqwest::blocking::Response, cap: u64) -> Result<String, String> {
    use std::io::Read as _;
    let mut bytes = Vec::new();
    resp.take(cap + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > cap {
        return Err(format!("the response is larger than {} KB", cap >> 10));
    }
    String::from_utf8(bytes).map_err(|_| "the response is not text".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A catalogue's models come in three shapes, and each is read: a bare
    /// name, one file, and several files with suffixes saying how each
    /// differs. The site's own tools and files that would leave the data
    /// folder are skipped.
    #[test]
    fn a_catalogue_is_read_in_each_shape() {
        let book = r#"[
            {"name": "_EQ", "phones": [{"name": "Sandbox", "file": "EQ sandbox"}]},
            {"name": "Moondrop", "phones": ["Aria", {"name": "Blessing 3", "file": "Moondrop Blessing 3"}]},
            {"name": "Aful", "phones": [
                {"name": "Performer 8S", "file": ["Aful Performer 8S", "Aful Performer 8S deep", "../escape"],
                 "suffix": ["", "(insert: deep)", ""]},
                {"name": "Cantor", "file": ["Aful Cantor", "Aful Cantor deep"]}
            ]}
        ]"#;
        let entries = parse_book(book).unwrap();
        let names: Vec<_> = entries
            .iter()
            .map(|(b, m, v, f)| format!("{b}|{m}|{v}|{f}"))
            .collect();
        assert_eq!(
            names,
            [
                "Moondrop|Aria||Aria",
                "Moondrop|Blessing 3||Moondrop Blessing 3",
                "Aful|Performer 8S||Aful Performer 8S",
                "Aful|Performer 8S|(insert: deep)|Aful Performer 8S deep",
                "Aful|Cantor||Aful Cantor",
                "Aful|Cantor|deep|Aful Cantor deep",
            ]
        );
        assert!(parse_book("<html>not here</html>").is_err());
        assert!(parse_book("[]").is_err());
    }

    /// A file's URL keeps its spaces and brackets as the site names them,
    /// escaped, inside the data folder.
    #[test]
    fn a_measurement_is_found_in_the_data_folder() {
        let url = data_url(&SITES[2], "Aful Performer 8S (deep) L.txt").unwrap();
        assert_eq!(
            url.as_str(),
            "https://graph.hangout.audio/iem/711/data/Aful%20Performer%208S%20(deep)%20L.txt"
        );
    }

    #[test]
    fn words_are_letters_and_digits() {
        assert_eq!(words_of("Aful  Performer-8S!"), ["aful", "performer", "8s"]);
    }
}
