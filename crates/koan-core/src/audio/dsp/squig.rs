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
    /// Where its `data/` folder is, and its catalogue in it.
    pub base: &'static str,
    /// The rig, where the site says which.
    pub rig: Option<&'static str>,
    /// In-ear or over-ear, where the site keeps one kind.
    pub ear: Option<Ear>,
    /// The folder under `base` its measurements are in.
    pub measurements: &'static str,
    /// How many samples it keeps of each side, as `<file> L1.txt` onwards;
    /// 0 where each side is one file, `<file> L.txt`.
    pub samples: u8,
    /// Why its measurements cannot be fetched, where they cannot: its
    /// catalogue is still searched, so a person learns it was measured there.
    pub locked: Option<&'static str>,
}

/// The sites searched: each answers with a catalogue in the shared layout.
pub const SITES: &[Site] = &[
    at("https://squig.link", None, None),
    Site {
        samples: 3,
        ..at("https://squig.link/headphones", None, Some(Ear::Over))
    },
    hangout("https://graph.hangout.audio/iem/711", Some("711"), Ear::In),
    hangout(
        "https://graph.hangout.audio/iem/5128",
        Some("5128"),
        Ear::In,
    ),
    hangout("https://graph.hangout.audio/headphones", None, Ear::Over),
    at("https://precog.squig.link", None, None),
    at("https://timmyv.squig.link", None, None),
    at("https://hbb.squig.link", None, None),
    at("https://kr0mka.squig.link", None, None),
    at("https://tgx78.squig.link", None, None),
    at("https://bakkwatan.squig.link", None, None),
    at("https://jaytiss.squig.link", None, None),
    // modernGraphTool keeps measurements apart from targets.
    Site {
        measurements: "data/phones",
        ..at("https://doltonius.squig.link", None, None)
    },
    Site {
        samples: 5,
        ..at("https://listener.squig.link", None, None)
    },
    at("https://pw.squig.link", None, None),
    at("https://kazi.squig.link", None, None),
    at("https://achoreviews.squig.link", None, None),
];

const fn at(base: &'static str, rig: Option<&'static str>, ear: Option<Ear>) -> Site {
    Site {
        base,
        rig,
        ear,
        measurements: "data",
        samples: 0,
        locked: None,
    }
}

/// Crinacle's sites refuse measurement files to anything but their own
/// pages, which decrypt them in the browser.
const fn hangout(base: &'static str, rig: Option<&'static str>, ear: Ear) -> Site {
    Site {
        locked: Some(
            "graph.hangout.audio does not let apps download its measurements; open it in a browser to view them",
        ),
        ..at(base, rig, Some(ear))
    }
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

/// Something kept for each site, by its base.
type BySite<T> = Mutex<Option<HashMap<&'static str, T>>>;

const FRESH_FOR: Duration = Duration::from_secs(24 * 60 * 60);
/// The largest catalogue accepted. The largest is about 700 KB.
const BOOK_CAP: u64 = 4 << 20;
/// The largest measurement accepted. They are about 15 KB.
const MEASUREMENT_CAP: u64 = 512 << 10;

/// Each site's catalogue as last read, and when.
static READ: BySite<(SystemTime, Arc<Vec<Entry>>)> = Mutex::new(None);

/// Every site's measurements whose name has each word of `query`, best
/// first: the model named exactly, then the shortest names, and those that
/// cannot be fetched last. A site that cannot be read is left out; an error
/// only where none can be.
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
            let rank = if site.locked.is_some() { 100_000 } else { 0 }
                + if exact { 0 } else { 1_000 }
                + text.len() * 10
                + variant.len();
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
/// the site has only one. An error names the site and what went wrong.
pub fn fetch(hit: &Hit) -> Result<String, String> {
    if let Some(why) = hit.site.locked {
        return Err(why.to_owned());
    }
    let client = http()?;
    let files = |side: &str| -> Vec<String> {
        if hit.site.samples == 0 {
            vec![format!("{} {side}.txt", hit.file)]
        } else {
            (1..=hit.site.samples)
                .map(|n| format!("{} {side}{n}.txt", hit.file))
                .collect()
        }
    };
    let names: Vec<String> = files("L").into_iter().chain(files("R")).collect();
    let read: Vec<Result<super::targets::Curve, String>> = std::thread::scope(|s| {
        let reads: Vec<_> = names
            .iter()
            .map(|name| s.spawn(|| curve(&client, hit, name)))
            .collect();
        reads
            .into_iter()
            .map(|r| r.join().unwrap_or_else(|_| Err("the read failed".into())))
            .collect()
    });
    let (left, right) = read.split_at(names.len() / 2);
    let side = |reads: &[Result<super::targets::Curve, String>]| {
        let curves: Vec<_> = reads.iter().filter_map(|r| r.as_ref().ok()).collect();
        match curves.split_first() {
            Some((first, rest)) => Ok(rest
                .iter()
                .enumerate()
                .fold((*first).clone(), |mean, (i, c)| {
                    average_weighted(&mean, c, i + 1)
                })),
            None => Err(reads
                .iter()
                .find_map(|r| r.as_ref().err())
                .cloned()
                .unwrap_or_default()),
        }
    };
    let curve = match (side(left), side(right)) {
        (Ok(l), Ok(r)) => average(&l, &r),
        (Ok(one), Err(e)) | (Err(e), Ok(one)) => {
            log::info!("squig: {} on one side only: {e}", hit.name());
            one
        }
        (Err(e), Err(_)) => return Err(e),
    };
    let curve = match calibration(&client, hit.site)? {
        Some(cal) => calibrated(&curve, &cal),
        None => curve,
    };
    let mut text = String::from("frequency,raw\n");
    for (hz, db) in curve {
        text.push_str(&format!("{hz:.3},{db:.3}\n"));
    }
    Ok(text)
}

/// The calibration `site` subtracts from every measurement before it draws
/// or equalises it, as its `config.js` names it in
/// `measurement_calibration_file`: a file beside its measurements. None
/// where it names none, or has no `config.js` to name one in. A
/// measurement without it would not be the one the site's presets were
/// made from.
fn calibration(
    client: &reqwest::blocking::Client,
    site: &Site,
) -> Result<Option<super::targets::Curve>, String> {
    let label = site.label();
    let config = url::Url::parse(&format!("{}/config.js", site.base)).map_err(|e| e.to_string())?;
    let resp = match client.get(config).send() {
        Ok(resp) if resp.status().is_success() => resp,
        Ok(resp) => {
            log::info!("squig: {label} has no config.js ({})", resp.status());
            return Ok(None);
        }
        Err(e) => return Err(format!("{label} could not be reached: {}", e.without_url())),
    };
    let Some(name) = calibration_named(&body(resp, BOOK_CAP).map_err(|e| format!("{label}: {e}"))?)
    else {
        return Ok(None);
    };
    let file = if name.ends_with(".txt") {
        name
    } else {
        format!("{name}.txt")
    };
    let resp = client
        .get(measurement_url(site, &file)?)
        .send()
        .map_err(|e| format!("{label} could not be reached: {}", e.without_url()))?;
    if !resp.status().is_success() {
        return Err(format!(
            "{label} calibrates its measurements with {file}, which it did not serve ({})",
            resp.status()
        ));
    }
    let cal =
        super::targets::points(&body(resp, MEASUREMENT_CAP).map_err(|e| format!("{label}: {e}"))?);
    if cal.len() < 20 {
        return Err(format!("{label}'s calibration {file} is not a curve"));
    }
    Ok(Some(cal))
}

/// The calibration file a site's `config.js` names, if it names one: the
/// last assignment to `measurement_calibration_file` that is not commented
/// out. Only a file in the site's own measurement folder is taken.
fn calibration_named(config: &str) -> Option<String> {
    let name = config
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with("//"))
        .filter_map(|l| {
            l.strip_prefix("measurement_calibration_file")?
                .trim_start()
                .strip_prefix('=')
        })
        .filter_map(|rest| {
            let rest = rest.trim_start();
            let quote = rest.chars().next().filter(|c| matches!(c, '"' | '\''))?;
            let rest = &rest[1..];
            Some(rest[..rest.find(quote)?].trim().to_owned())
        })
        .next_back()?;
    if name.is_empty() {
        return None;
    }
    if !stays_in(&name) {
        log::warn!("squig: a calibration outside the site's data is not taken: {name}");
        return None;
    }
    Some(name)
}

/// `curve` with `calibration` subtracted, on the measurement's own
/// frequencies.
pub(crate) fn calibrated(
    curve: &[(f64, f64)],
    calibration: &[(f64, f64)],
) -> super::targets::Curve {
    curve
        .iter()
        .map(|&(hz, db)| (hz, db - super::targets::at(calibration, hz)))
        .collect()
}

/// One measurement file of `hit`'s, read as a curve.
fn curve(
    client: &reqwest::blocking::Client,
    hit: &Hit,
    name: &str,
) -> Result<super::targets::Curve, String> {
    let site = hit.site.label();
    let resp = client
        .get(measurement_url(hit.site, name)?)
        .send()
        .map_err(|e| format!("{site} could not be reached: {}", e.without_url()))?;
    match resp.status() {
        s if s.is_success() => {}
        reqwest::StatusCode::NOT_FOUND => {
            return Err(format!("{site} has no measurement file for {}", hit.name()));
        }
        s => return Err(format!("{site} refused the measurement ({s})")),
    }
    let curve =
        super::targets::points(&body(resp, MEASUREMENT_CAP).map_err(|e| format!("{site}: {e}"))?);
    if curve.len() < 20 {
        return Err(format!(
            "{site} answered with something that is not a measurement of {}",
            hit.name()
        ));
    }
    Ok(curve)
}

/// The mean of `n` curves averaged so far and one more, by amplitude.
fn average_weighted(mean: &[(f64, f64)], next: &[(f64, f64)], n: usize) -> super::targets::Curve {
    let amplitude = |db: f64| 10f64.powf(db / 20.0);
    let n = n as f64;
    mean.iter()
        .map(|&(hz, db)| {
            let r = super::targets::at(next, hz);
            (
                hz,
                20.0 * ((amplitude(db) * n + amplitude(r)) / (n + 1.0)).log10(),
            )
        })
        .collect()
}

/// Two channels' levels as one, as squig.link's graphs average them: the
/// mean of their amplitudes, in dB again.
pub(crate) fn average(left: &[(f64, f64)], right: &[(f64, f64)]) -> super::targets::Curve {
    average_weighted(left, right, 1)
}

/// A file name that stays in a site's data folder.
fn stays_in(name: &str) -> bool {
    !name.is_empty() && !name.contains(['/', '\\']) && !name.starts_with('.')
}

/// `name` in `site`'s data folder, as a URL. Refused for a name that would
/// leave it.
fn data_url(site: &Site, name: &str) -> Result<url::Url, String> {
    file_url(site, "data", name)
}

/// The measurement file `name` on `site`.
fn measurement_url(site: &Site, name: &str) -> Result<url::Url, String> {
    file_url(site, site.measurements, name)
}

fn file_url(site: &Site, folder: &str, name: &str) -> Result<url::Url, String> {
    if !stays_in(name) {
        return Err(format!("{name} is not a file in a squig.link site's data"));
    }
    let mut url =
        url::Url::parse(&format!("{}/{folder}/", site.base)).map_err(|e| e.to_string())?;
    url.path_segments_mut()
        .map_err(|()| format!("{} is not a site", site.base))?
        .pop_if_empty()
        .push(name);
    Ok(url)
}

/// A site being read, so a second search waits for the first rather than
/// asking the site again.
static READING: BySite<Arc<Mutex<()>>> = Mutex::new(None);
/// When a site last failed to answer, with no copy kept to fall back on.
static FAILED: BySite<SystemTime> = Mutex::new(None);
/// How long a site that did not answer is left alone.
const RETRY_AFTER: Duration = Duration::from_secs(5 * 60);

/// `site`'s catalogue as read this last day, if it was.
fn read_lately(site: &Site) -> Option<Arc<Vec<Entry>>> {
    let read = READ.lock();
    let (at, entries) = read.as_ref()?.get(site.base)?;
    SystemTime::now()
        .duration_since(*at)
        .is_ok_and(|age| age < FRESH_FOR)
        .then(|| entries.clone())
}

/// `site`'s catalogue: the copy read this last day, the one kept beside the
/// config while it is a day old, or fetched. A copy kept, however old, is
/// used when the site does not answer; one that did not answer, with none
/// kept, is not asked again for a few minutes. One read of a site at a time.
fn book(site: &'static Site) -> Result<Arc<Vec<Entry>>, String> {
    if let Some(entries) = read_lately(site) {
        return Ok(entries);
    }
    let one = READING
        .lock()
        .get_or_insert_with(HashMap::new)
        .entry(site.base)
        .or_default()
        .clone();
    let _one = one.lock();
    // Read while this waited.
    if let Some(entries) = read_lately(site) {
        return Ok(entries);
    }
    let file = cache_dir().join(format!("{}.json", slug(site.base)));
    let kept = std::fs::read_to_string(&file).ok();
    let fresh = std::fs::metadata(&file)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age < FRESH_FOR);
    let kept_entries = kept.as_deref().map(parse_book);
    let failed_lately = FAILED
        .lock()
        .as_ref()
        .and_then(|f| f.get(site.base))
        .and_then(|at| SystemTime::now().duration_since(*at).ok())
        .is_some_and(|ago| ago < RETRY_AFTER);
    if failed_lately && !matches!(kept_entries, Some(Ok(_))) {
        return Err(format!("{} did not answer a few minutes ago", site.label()));
    }
    let entries = match kept_entries {
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
                    FAILED
                        .lock()
                        .get_or_insert_with(HashMap::new)
                        .insert(site.base, SystemTime::now());
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
                if !stays_in(file) {
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

/// HTTPS, following a redirect only within the host first asked: a site in
/// the list never sends koan elsewhere, let alone to the local network.
fn http() -> Result<reqwest::blocking::Client, String> {
    let policy = reqwest::redirect::Policy::custom(|attempt| {
        let same = attempt
            .previous()
            .first()
            .is_some_and(|first| first.host_str() == attempt.url().host_str());
        if same && attempt.previous().len() < 5 {
            attempt.follow()
        } else {
            attempt.stop()
        }
    });
    reqwest::blocking::Client::builder()
        .https_only(true)
        .redirect(policy)
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
    within(bytes, cap)
}

fn within(bytes: Vec<u8>, cap: u64) -> Result<String, String> {
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
    /// escaped, inside the site's measurements folder.
    #[test]
    fn a_measurement_is_found_in_the_data_folder() {
        let url = measurement_url(&SITES[0], "Aful Performer 8S (deep) L.txt").unwrap();
        assert_eq!(
            url.as_str(),
            "https://squig.link/data/Aful%20Performer%208S%20(deep)%20L.txt"
        );
        let doltonius = site("https://doltonius.squig.link").unwrap();
        assert_eq!(
            measurement_url(doltonius, "Zero 2 (Stock M) L.txt")
                .unwrap()
                .as_str(),
            "https://doltonius.squig.link/data/phones/Zero%202%20(Stock%20M)%20L.txt"
        );
        assert_eq!(
            data_url(doltonius, "phone_book.json").unwrap().as_str(),
            "https://doltonius.squig.link/data/phone_book.json"
        );
    }

    /// graph.hangout.audio answers 403 to a measurement file, whatever asks:
    /// its results come last and say why before anything is fetched.
    #[test]
    fn a_locked_site_is_listed_last_and_refused_up_front() {
        let s5128 = site("https://graph.hangout.audio/iem/5128").unwrap();
        let hit = Hit {
            site: s5128,
            brand: "AFUL".into(),
            model: "Performer 8S".into(),
            variant: String::new(),
            file: "Performer 8S".into(),
        };
        let err = fetch(&hit).unwrap_err();
        assert!(err.contains("graph.hangout.audio"), "{err}");
        assert!(
            SITES
                .iter()
                .filter(|s| s.locked.is_some())
                .all(|s| s.base.contains("hangout"))
        );
    }

    /// Samples of a side average by amplitude, each counted once.
    #[test]
    fn samples_average_evenly() {
        let a = [(100.0, 0.0)];
        let b = [(100.0, 0.0)];
        let c = [(100.0, 20.0)];
        let ab = average_weighted(&a, &b, 1);
        let abc = average_weighted(&ab, &c, 2);
        let expected = 20.0 * ((1.0 + 1.0 + 10.0) / 3.0f64).log10();
        assert!((abc[0].1 - expected).abs() < 1e-9, "{}", abc[0].1);
    }

    /// Two sides averaged as squig.link's graphs average them: by
    /// amplitude, so 0 dB and 6 dB make 3.5, not 3.
    #[test]
    fn sides_average_by_amplitude() {
        let left = [(100.0, 0.0), (1000.0, 0.0)];
        let right = [(100.0, 6.0), (1000.0, 0.0)];
        let both = average(&left, &right);
        let expected = 20.0 * ((1.0 + 10f64.powf(6.0 / 20.0)) / 2.0).log10();
        assert!((both[0].1 - expected).abs() < 1e-9);
        assert!((both[0].1 - 3.51).abs() < 0.01, "{}", both[0].1);
        assert_eq!(both[1].1, 0.0);
    }

    /// squig.link names its calibration in `config.js`, after a commented-out
    /// line naming none; it is subtracted from the measurement on the
    /// measurement's own frequencies. kazi.squig.link names none.
    #[test]
    fn a_named_calibration_is_subtracted() {
        let squig = "      extraMusicEnabled = true,\n//      measurement_calibration_file = \"\";\n      measurement_calibration_file = \"IEF 2023 Cal\";\n";
        assert_eq!(calibration_named(squig).as_deref(), Some("IEF 2023 Cal"));
        assert_eq!(calibration_named("const DIR = \"data/\";\n"), None);
        assert_eq!(
            calibration_named("measurement_calibration_file = \"\",\n"),
            None
        );
        assert_eq!(
            calibration_named("measurement_calibration_file = '../secret',\n"),
            None
        );
        let text = include_str!("testdata/squig-ief-2023-cal.txt");
        let cal = crate::audio::dsp::targets::points(text);
        let flat: Vec<(f64, f64)> = [20.0, 2000.0, 10_000.0]
            .into_iter()
            .map(|hz| (hz, 90.0))
            .collect();
        let got = calibrated(&flat, &cal);
        for ((hz, db), (_, was)) in got.iter().zip(&flat) {
            let c = crate::audio::dsp::targets::at(&cal, *hz);
            assert!((db - (was - c)).abs() < 1e-9);
        }
        // The IEF 2023 calibration's shape: a dip at 2 kHz, a lift above 8.
        let k = crate::audio::dsp::targets::at(&cal, 1000.0);
        assert!(crate::audio::dsp::targets::at(&cal, 2000.0) - k < -1.0);
        assert!(crate::audio::dsp::targets::at(&cal, 10_000.0) - k > 1.5);
    }

    /// A response past its cap is refused, not cut short.
    #[test]
    fn a_body_past_its_cap_is_refused() {
        assert_eq!(within(b"[]".to_vec(), 2).as_deref(), Ok("[]"));
        assert!(within(b"[1]".to_vec(), 2).is_err());
        assert!(within(vec![0xff, 0xfe], 8).is_err(), "not text");
    }

    /// A site that does not answer: its copy kept, however old, is used; with
    /// none kept, it is left out and not asked again for a few minutes.
    #[test]
    fn a_site_that_does_not_answer() {
        let _guard = crate::config::tests::PERSIST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        config::set_config_dir(dir.path());
        // Nothing listens on the discard port.
        let kept: &'static Site = Box::leak(Box::new(at("https://127.0.0.1:9/kept", None, None)));
        let none: &'static Site = Box::leak(Box::new(at("https://127.0.0.1:9/none", None, None)));
        std::fs::create_dir_all(cache_dir()).unwrap();
        let file = cache_dir().join(format!("{}.json", slug(kept.base)));
        std::fs::write(&file, r#"[{"name": "Aful", "phones": ["Cantor"]}]"#).unwrap();
        let old = SystemTime::now() - Duration::from_secs(3 * 24 * 60 * 60);
        std::fs::File::options()
            .append(true)
            .open(&file)
            .unwrap()
            .set_modified(old)
            .unwrap();
        assert_eq!(book(kept).unwrap()[0].1, "Cantor", "the copy kept");

        assert!(book(none).is_err());
        let again = book(none).unwrap_err();
        assert!(
            again.contains("did not answer a few minutes ago"),
            "{again}"
        );
    }

    /// A name from outside a catalogue cannot leave the data folder.
    #[test]
    fn a_name_cannot_leave_the_data_folder() {
        for name in ["../config.toml", "a/b L.txt", ".hidden", ""] {
            assert!(data_url(&SITES[0], name).is_err(), "{name}");
        }
    }

    #[test]
    fn words_are_letters_and_digits() {
        assert_eq!(words_of("Aful  Performer-8S!"), ["aful", "performer", "8s"]);
    }
}
