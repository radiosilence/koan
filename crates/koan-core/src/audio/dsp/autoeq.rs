//! AutoEQ's headphone corrections, found by name rather than by file.
//!
//! AutoEQ publishes an index of every result it has, one line per headphone
//! and measurement source. koan keeps a copy beside its config, asks for a new
//! one at most once a day (by ETag, so an unchanged index costs a 304), and
//! falls back to the copy when GitHub cannot be reached. Installing an entry
//! fetches its `ParametricEQ.txt` and saves it through the same import as a
//! file handed over by hand.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use super::import::{self, Imported};
use super::profiles;
use crate::config;

const RESULTS: &str = "https://raw.githubusercontent.com/jaakkopasanen/AutoEq/master/results";

/// How long a fetched index is used before asking whether it changed.
const FRESH_FOR: Duration = Duration::from_secs(24 * 60 * 60);

/// One result in AutoEQ's index.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// Its place in the index, from 1: what `koan dsp autoeq install` takes.
    pub number: usize,
    /// The headphone, as AutoEQ names it.
    pub name: String,
    /// Who measured it.
    pub source: String,
    /// The measurement rig, where the source used more than one.
    pub rig: Option<String>,
    /// The result's folder under `results/`, URL-encoded as the index has it.
    path: String,
}

impl Entry {
    /// The source and rig, as the index words them.
    pub fn measured_by(&self) -> String {
        match &self.rig {
            Some(rig) => format!("{} on {rig}", self.source),
            None => self.source.clone(),
        }
    }

    /// What a profile installed from it is called.
    pub fn profile_name(&self) -> String {
        format!("{} (AutoEQ, {})", self.name, self.measured_by())
    }

    /// Where its ParametricEQ.txt is: always under [`RESULTS`], or `None`.
    fn parametric_url(&self) -> Option<String> {
        if !safe_path(&self.path) {
            return None;
        }
        let folder = self.path.rsplit('/').next().unwrap_or_default();
        let url = url::Url::parse(&format!(
            "{RESULTS}/{}/{folder}%20ParametricEQ.txt",
            self.path
        ))
        .ok()?;
        url.as_str()
            .starts_with(&format!("{RESULTS}/"))
            .then(|| url.into())
    }
}

/// An index path that stays inside `results/`: relative, without `.` or `..`
/// segments however they are spelled, and nothing that would end the path
/// (`?`, `#`) or be read as a separator (`\`, an encoded `/`).
fn safe_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains(['?', '#', '\\'])
        && !lower.contains("%2f")
        && !lower.contains("%5c")
        && lower.split('/').all(|segment| {
            let segment = segment.replace("%2e", ".");
            !segment.is_empty() && segment != "." && segment != ".."
        })
}

/// Read AutoEQ's `results/INDEX.md`: lines such as
/// `- [Sennheiser HD 650](./crinacle/GRAS%2043AG-7%20over-ear/Sennheiser%20HD%20650) by crinacle on GRAS 43AG-7`.
/// Anything else is skipped. Within one name, AutoEQ lists its preferred
/// source first, and the order is kept.
pub fn parse_index(text: &str) -> Vec<Entry> {
    text.lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("- [")?;
            let (name, rest) = rest.split_once("](./")?;
            let (path, by) = rest.rsplit_once(") by ")?;
            let (source, rig) = match by.split_once(" on ") {
                Some((source, rig)) => (source, Some(rig.trim().to_owned())),
                None => (by, None),
            };
            safe_path(path).then_some((name, path, source, rig))
        })
        .enumerate()
        .map(|(i, (name, path, source, rig))| Entry {
            number: i + 1,
            name: name.to_owned(),
            source: source.trim().to_owned(),
            rig,
            path: path.trim_end_matches('/').to_owned(),
        })
        .collect()
}

/// The entries best matching `query`, fuzzily, at most `limit`. Ties go to
/// the shorter name, then to the index's own order, so a model comes before
/// its variants and AutoEQ's preferred source before the others.
pub fn search<'a>(entries: &'a [Entry], query: &str, limit: usize) -> Vec<&'a Entry> {
    use nucleo::pattern::{CaseMatching, Normalization, Pattern};
    use nucleo::{Config, Matcher, Utf32Str};

    let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut buf = Vec::new();
    let mut scored: Vec<(u32, &Entry)> = entries
        .iter()
        .filter_map(|e| {
            pattern
                .score(Utf32Str::new(&e.name, &mut buf), &mut matcher)
                .map(|score| (score, e))
        })
        .collect();
    scored.sort_by_key(|&(score, e)| (std::cmp::Reverse(score), e.name.len(), e.number));
    scored.into_iter().take(limit).map(|(_, e)| e).collect()
}

/// The entry `wanted` names: its number in the index, or its name exactly
/// (case aside), from `source` if given, else AutoEQ's preferred source.
pub fn find<'a>(entries: &'a [Entry], wanted: &str, source: Option<&str>) -> Option<&'a Entry> {
    let wanted = wanted.trim();
    if let Ok(n) = wanted.parse::<usize>() {
        return entries.iter().find(|e| e.number == n);
    }
    entries.iter().find(|e| {
        e.name.eq_ignore_ascii_case(wanted)
            && source.is_none_or(|s| {
                e.source.eq_ignore_ascii_case(s) || e.measured_by().eq_ignore_ascii_case(s)
            })
    })
}

fn cache_dir() -> PathBuf {
    config::config_dir().join("autoeq")
}

fn http() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent(concat!("koan/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())
}

/// How far [`index`] may go to the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    /// The copy kept while it is less than a day old, otherwise asked for
    /// again: what searching uses.
    Daily,
    /// Asked for whatever the copy's age.
    Refresh,
    /// The copy kept, however old, asked for only when there is none: what
    /// installing by a number from an earlier search uses, since a newer
    /// index may number things differently.
    Kept,
}

/// The largest index accepted. It is about 850 KB.
const INDEX_CAP: u64 = 8 << 20;
/// The largest ParametricEQ.txt accepted. They are about 1 KB.
const PARAMETRIC_CAP: u64 = 64 << 10;

/// AutoEQ's index, as `freshness` allows. The copy kept is used whenever
/// GitHub does not answer, or answers with something that is not an index.
pub fn index(freshness: Freshness) -> Result<Vec<Entry>, String> {
    let dir = cache_dir();
    let file = dir.join("INDEX.md");
    let etag_file = dir.join("INDEX.etag");
    let kept = std::fs::read_to_string(&file).ok();
    let age = std::fs::metadata(&file)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok());
    if let Some(text) = &kept {
        let fresh = match freshness {
            Freshness::Daily => age.is_some_and(|a| a < FRESH_FOR),
            Freshness::Refresh => false,
            Freshness::Kept => true,
        };
        if fresh {
            return Ok(parse_index(text));
        }
    }

    let etag = kept
        .as_ref()
        .and_then(|_| std::fs::read_to_string(&etag_file).ok());
    let fetched = fetch_index(etag.as_deref()).and_then(|f| match f {
        Fetched::New { text, .. } if parse_index(&text).is_empty() => {
            Err("the index fetched lists nothing".to_owned())
        }
        f => Ok(f),
    });
    match fetched {
        Ok(Fetched::Unchanged) => {
            // Fresh again for another day.
            let _ = std::fs::File::options()
                .append(true)
                .open(&file)
                .and_then(|f| f.set_modified(SystemTime::now()));
            Ok(parse_index(kept.as_deref().unwrap_or_default()))
        }
        Ok(Fetched::New { text, etag }) => {
            // Written beside it and renamed over it, so an interrupted write
            // never leaves a truncated index looking fresh.
            let part = dir.join("INDEX.md.part");
            let kept_new = std::fs::create_dir_all(&dir)
                .and_then(|()| std::fs::write(&part, &text))
                .and_then(|()| std::fs::rename(&part, &file));
            if let Err(e) = &kept_new {
                log::info!("autoeq: index not kept: {e}");
                let _ = std::fs::remove_file(&part);
            }
            match etag {
                Some(etag) if kept_new.is_ok() => {
                    let _ = std::fs::write(&etag_file, etag);
                }
                _ => {
                    let _ = std::fs::remove_file(&etag_file);
                }
            }
            Ok(parse_index(&text))
        }
        Err(e) => match kept {
            Some(text) => {
                log::info!("autoeq: index not refreshed, using the copy kept: {e}");
                Ok(parse_index(&text))
            }
            None => Err(format!("AutoEQ's index could not be fetched: {e}")),
        },
    }
}

enum Fetched {
    Unchanged,
    New { text: String, etag: Option<String> },
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

fn fetch_index(etag: Option<&str>) -> Result<Fetched, String> {
    let mut request = http()?.get(format!("{RESULTS}/INDEX.md"));
    if let Some(etag) = etag {
        request = request.header(reqwest::header::IF_NONE_MATCH, etag.trim());
    }
    let resp = request.send().map_err(|e| e.without_url().to_string())?;
    if resp.status() == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(Fetched::Unchanged);
    }
    if !resp.status().is_success() {
        return Err(format!("GitHub answered {}", resp.status()));
    }
    let etag = resp
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let text = body(resp, INDEX_CAP)?;
    Ok(Fetched::New { text, etag })
}

/// `entry`'s parametric EQ, as AutoEQ writes it, made into a profile import
/// named for the headphone and who measured it.
pub fn imported(entry: &Entry, parametric: &str) -> Result<Imported, String> {
    let mut imported = import::import_text(parametric, None).map_err(|e| e.to_string())?;
    if imported.filters.is_empty() {
        return Err(format!("AutoEQ's file for {} holds no EQ", entry.name));
    }
    imported.name = entry.profile_name();
    imported.source = vec![format!("AutoEQ: {} ParametricEQ.txt", entry.name)];
    Ok(imported)
}

/// Fetch `entry`'s parametric EQ and save it as a profile. Answers with the
/// profile's name. A profile of that name already there is replaced, keeping
/// its devices.
pub fn install(entry: &Entry) -> Result<String, String> {
    let url = entry
        .parametric_url()
        .ok_or_else(|| format!("AutoEQ's index gives {} an address outside it", entry.name))?;
    let resp = http()?
        .get(url)
        .send()
        .map_err(|e| format!("AutoEQ could not be reached: {}", e.without_url()))?;
    if !resp.status().is_success() {
        return Err(format!(
            "AutoEQ has no parametric EQ for {} ({})",
            entry.name,
            resp.status()
        ));
    }
    let text =
        body(resp, PARAMETRIC_CAP).map_err(|e| format!("AutoEQ's file for {}: {e}", entry.name))?;
    profiles::save(imported(entry, &text)?, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    const INDEX: &str = "# Index
This is a list of all equalization profiles. Target is in parentheses if there are results with multiple targets
from the same source.

- [1MORE Aero (ANC Off)](./HypetheSonics/GRAS%20RA0045%20in-ear/1MORE%20Aero%20(ANC%20Off)) by HypetheSonics on GRAS RA0045
- [Sennheiser HD 600](./oratory1990/over-ear/Sennheiser%20HD%20600) by oratory1990
- [Sennheiser HD 650](./oratory1990/over-ear/Sennheiser%20HD%20650) by oratory1990
- [Sennheiser HD 650](./crinacle/GRAS%2043AG-7%20over-ear/Sennheiser%20HD%20650) by crinacle on GRAS 43AG-7
- [Sennheiser HD 650 (2020)](./Innerfidelity/over-ear/Sennheiser%20HD%20650%20(2020)) by Innerfidelity
- [Sennheiser HD 660 S](./oratory1990/over-ear/Sennheiser%20HD%20660%20S) by oratory1990
";

    const PARAMETRIC: &str = "Preamp: -6.1 dB
Filter 1: ON LSC Fc 105 Hz Gain 6.4 dB Q 0.70
Filter 2: ON PK Fc 8800 Hz Gain 5.1 dB Q 1.42
Filter 3: ON PK Fc 118 Hz Gain -3.1 dB Q 0.50
";

    #[test]
    fn the_index_is_read_line_by_line() {
        let entries = parse_index(INDEX);
        assert_eq!(entries.len(), 6);
        let aero = &entries[0];
        assert_eq!(aero.number, 1);
        assert_eq!(aero.name, "1MORE Aero (ANC Off)");
        assert_eq!(aero.source, "HypetheSonics");
        assert_eq!(aero.rig.as_deref(), Some("GRAS RA0045"));
        assert_eq!(
            aero.parametric_url().unwrap(),
            format!(
                "{RESULTS}/HypetheSonics/GRAS%20RA0045%20in-ear/1MORE%20Aero%20(ANC%20Off)/1MORE%20Aero%20(ANC%20Off)%20ParametricEQ.txt"
            )
        );
        assert_eq!(entries[2].rig, None);
        assert_eq!(
            entries[3].profile_name(),
            "Sennheiser HD 650 (AutoEQ, crinacle on GRAS 43AG-7)"
        );
    }

    #[test]
    fn an_index_line_cannot_point_outside_autoeq() {
        let hostile = [
            "- [A](./../../../../other/repo/main/x) by a",
            "- [B](./oratory1990/%2E%2e/%2e%2E/x) by b",
            "- [C](./oratory1990/over-ear/C?x=1) by c",
            "- [D](./oratory1990/over-ear/D#x) by d",
            "- [E](./oratory1990\\..\\x) by e",
            "- [F](.//etc/x) by f",
            "- [G](./a%2F..%2F..%2Fx) by g",
            "- [H](./oratory1990/./x) by h",
        ];
        for line in hostile {
            assert!(parse_index(line).is_empty(), "{line}");
        }
        let fine = parse_index("- [I](./oratory1990/over-ear/I%20(2020)) by i");
        assert!(fine[0].parametric_url().unwrap().starts_with(RESULTS));
    }

    #[test]
    fn a_kept_index_is_read_as_it_is_however_old() {
        let _guard = crate::config::tests::PERSIST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        config::set_config_dir(dir.path());
        let kept = dir.path().join("autoeq/INDEX.md");
        std::fs::create_dir_all(kept.parent().unwrap()).unwrap();
        std::fs::write(&kept, INDEX).unwrap();
        let old = SystemTime::now() - FRESH_FOR * 30;
        std::fs::File::options()
            .append(true)
            .open(&kept)
            .unwrap()
            .set_modified(old)
            .unwrap();
        let entries = index(Freshness::Kept).unwrap();
        assert_eq!(entries.len(), 6);
        assert_eq!(
            std::fs::metadata(&kept).unwrap().modified().unwrap(),
            old,
            "read without asking GitHub"
        );
    }

    #[test]
    fn search_puts_the_closest_name_and_the_preferred_source_first() {
        let entries = parse_index(INDEX);
        let found = search(&entries, "hd650", 10);
        let names: Vec<(&str, &str)> = found
            .iter()
            .map(|e| (e.name.as_str(), e.source.as_str()))
            .collect();
        assert_eq!(
            names[..3],
            [
                ("Sennheiser HD 650", "oratory1990"),
                ("Sennheiser HD 650", "crinacle"),
                ("Sennheiser HD 650 (2020)", "Innerfidelity"),
            ]
        );
        assert!(search(&entries, "zzzz", 10).is_empty());
        assert_eq!(search(&entries, "sennheiser", 2).len(), 2);
    }

    #[test]
    fn an_entry_is_found_by_number_or_name_and_source() {
        let entries = parse_index(INDEX);
        assert_eq!(find(&entries, "4", None).unwrap().source, "crinacle");
        assert_eq!(
            find(&entries, "sennheiser hd 650", None).unwrap().source,
            "oratory1990"
        );
        assert_eq!(
            find(&entries, "Sennheiser HD 650", Some("crinacle"))
                .unwrap()
                .number,
            4
        );
        assert!(find(&entries, "Sennheiser HD 65", None).is_none());
        assert!(find(&entries, "99", None).is_none());
    }

    #[test]
    fn a_parametric_eq_installs_as_a_named_profile() {
        let _guard = crate::config::tests::PERSIST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        config::set_config_dir(dir.path());

        let entries = parse_index(INDEX);
        let entry = &entries[2];
        let name = profiles::save(imported(entry, PARAMETRIC).unwrap(), None).unwrap();
        assert_eq!(name, "Sennheiser HD 650 (AutoEQ, oratory1990)");
        let detail = profiles::detail(&name).unwrap();
        assert_eq!(detail.filters.len(), 3);
        assert!(detail.problem.is_none(), "{:?}", detail.problem);
        assert!(imported(entry, "Preamp: -1 dB\n").is_err());
    }
}
