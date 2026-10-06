//! AutoEQ's headphone corrections, found by name rather than by file.
//!
//! AutoEQ publishes an index of every result it has, one line per headphone
//! and measurement source. koan keeps a copy beside its config, asks for a new
//! one at most once a day (by ETag, so an unchanged index costs a 304), and
//! falls back to the copy when GitHub cannot be reached. Installing an entry
//! fetches its `ParametricEQ.txt` and saves it through the same import as a
//! file handed over by hand.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use parking_lot::Mutex;

use super::import::{self, Imported};
use super::profiles;
use crate::config::{self, Config};

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
    /// Where its result's CSV is: the measurement, the target and the
    /// corrections on AutoEQ's grid.
    fn csv_url(&self) -> Option<String> {
        let parametric = self.parametric_url()?;
        let folder = self.path.rsplit('/').next().unwrap_or_default();
        Some(parametric.replace(
            &format!("/{folder}%20ParametricEQ.txt"),
            &format!("/{folder}.csv"),
        ))
    }

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
/// The largest result CSV accepted. They are about 70 KB.
const RESULT_CAP: u64 = 1 << 20;

/// The index as last read, and when, so a search per keystroke does not
/// read and parse 850 KB each time.
static READ: Mutex<Option<(SystemTime, Arc<Vec<Entry>>)>> = Mutex::new(None);

/// AutoEQ's index, as `freshness` allows. The copy kept is used whenever
/// GitHub does not answer, or answers with something that is not an index.
pub fn index(freshness: Freshness) -> Result<Arc<Vec<Entry>>, String> {
    if let Some((at, entries)) = READ.lock().as_ref() {
        let fresh = match freshness {
            Freshness::Daily => SystemTime::now()
                .duration_since(*at)
                .is_ok_and(|age| age < FRESH_FOR),
            Freshness::Refresh => false,
            Freshness::Kept => true,
        };
        if fresh {
            return Ok(entries.clone());
        }
    }
    let entries = Arc::new(read_index(freshness)?);
    *READ.lock() = Some((SystemTime::now(), entries.clone()));
    Ok(entries)
}

fn read_index(freshness: Freshness) -> Result<Vec<Entry>, String> {
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

/// The words of a name, lowercased: runs of letters and digits, and `+` on
/// its own, so "Buds+" is not "Buds". `WH-1000XM4` → `wh`, `1000xm4`.
fn words(name: &str) -> Vec<String> {
    tokens(name).into_iter().map(|w| w.to_lowercase()).collect()
}

/// `words`, as the name spells them.
fn tokens(name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    for c in name.chars() {
        if c.is_alphanumeric() {
            word.push(c);
            continue;
        }
        if !word.is_empty() {
            out.push(std::mem::take(&mut word));
        }
        if c == '+' {
            out.push("+".to_owned());
        }
    }
    if !word.is_empty() {
        out.push(word);
    }
    out
}

/// Headphones whose model name alone, without the maker, says which one it
/// is, as their own Bluetooth names give it: "WH-1000XM4", "Jo's AirPods Max".
/// Each model is one AutoEQ entry, under exactly this name, and names its
/// generation; a model AutoEQ lists only by its modes (the AirPods Pro 2's
/// ANC and transparency) is not one, nor is one sold in versions that call
/// themselves alike (AirPods 4, with and without ANC). Left out on
/// purpose: plain "AirPods" and "AirPods Pro", which every generation calls
/// itself, so the name cannot say which correction fits; and models whose
/// devices go by an abbreviation the index does not use ("Bose QC45").
const MAKERLESS: &[&str] = &[
    "Apple AirPods Max",
    "Samsung Galaxy Buds2",
    "Samsung Galaxy Buds2 Pro",
    "Samsung Galaxy Buds3",
    "Samsung Galaxy Buds3 Pro",
    "Sony LinkBuds Fit",
    "Sony LinkBuds S",
    "Sony WF-1000XM3",
    "Sony WF-1000XM4",
    "Sony WF-1000XM5",
    "Sony WH-1000XM2",
    "Sony WH-1000XM3",
    "Sony WH-1000XM4",
    "Sony WH-1000XM5",
    "Sony WH-1000XM6",
];

/// The entry an output device is, judged from its name alone, or `None`
/// unless that is beyond doubt, since a wrong correction is worse than none.
/// The device's name must end with the headphone's whole name as AutoEQ
/// gives it, maker included, on word boundaries: "Jo's Sony WH-1000XM4" is
/// Sony's WH-1000XM4, while "MOTU M2" (Brainwavz M2) and "Hugo 2" (Ortofon 2)
/// are nothing. Ending it rules out a newer generation or a variant the index
/// lacks: "Apple AirPods Pro 3" is not the AirPods Pro. For the models in
/// `MAKERLESS` the name may leave the maker out, still ending with the whole
/// model. A one-word entry never matches. The longest name wins, and between
/// sources AutoEQ's preferred one.
pub fn suggest<'a>(entries: &'a [Entry], device: &str) -> Option<&'a Entry> {
    let device = words(device);
    let mut best: Option<(usize, &Entry)> = None;
    for e in entries {
        let name = words(&e.name);
        if name.len() < 2 {
            continue;
        }
        let matched = if device.ends_with(&name) {
            name.len()
        } else if MAKERLESS.contains(&e.name.as_str()) && device.ends_with(&name[1..]) {
            name.len() - 1
        } else {
            continue;
        };
        if best.is_none_or(|(n, _)| matched > n) {
            best = Some((matched, e));
        }
    }
    best.map(|(_, e)| e)
}

/// Words in an output's name that say it is not headphones, whatever else
/// the name has in it.
const NOT_HEADPHONE_WORDS: &[&str] = &[
    "airplay",
    "dac",
    "display",
    "displayport",
    "hdmi",
    "interface",
    "monitor",
    "monitors",
    "output",
    "soundbar",
    "soundlink",
    "soundtouch",
    "speaker",
    "speakers",
    "tv",
    "usb",
];

/// What to search AutoEQ for, for an output whose name says roughly which
/// headphone it is but not which entry: "AirPods Pro" for "Jo's AirPods Pro",
/// whose generations AutoEQ lists apart, or "Bose QuietComfort" for "Bose
/// QC45", which the index spells out. Offered where `suggest` offers nothing,
/// so the person picks the variant themselves.
///
/// The end of the name, up to four words, that begins some entry's model, the
/// longest that does; it must hold a word of four letters or more, and a
/// single word six, so "M2", "4i4" or "Solo" alone never count. Failing that,
/// a headphone line named by its abbreviation (`ABBREVIATED`). A name with a
/// word like "speakers", "display", "USB" or "SoundLink" in it is not
/// headphones and gets nothing.
pub fn search_for(entries: &[Entry], device: &str) -> Option<String> {
    let spelt = tokens(device);
    let device: Vec<String> = spelt.iter().map(|w| w.to_lowercase()).collect();
    if device
        .iter()
        .any(|w| NOT_HEADPHONE_WORDS.contains(&w.as_str()))
    {
        return None;
    }
    let models: Vec<Vec<String>> = entries
        .iter()
        .map(|e| {
            let maker = words(maker_of(&e.name)).len();
            words(&e.name).split_off(maker)
        })
        .collect();
    // A run of words needs one of four letters or more; a single word, six,
    // since a short one ("Solo", "Duet") begins a model of some maker's and
    // names an audio interface as often as a headphone.
    let lettered = |run: &[String]| {
        let letters = |w: &String| w.chars().filter(|c| c.is_alphabetic()).count();
        match run {
            [one] => letters(one) >= 6,
            _ => run.iter().any(|w| letters(w) >= 4),
        }
    };
    for len in (1..=device.len().min(4)).rev() {
        let run = &device[device.len() - len..];
        if lettered(run) && models.iter().any(|m| m.starts_with(run)) {
            return Some(spelt[spelt.len() - len..].join(" "));
        }
    }
    ABBREVIATED.iter().find_map(|(maker, starts, search)| {
        let rest = device.strip_prefix(&[maker.to_string()])?;
        rest.iter()
            .any(|w| starts.iter().any(|s| w.starts_with(s)))
            .then(|| (*search).to_owned())
    })
}

/// Headphone lines whose devices name themselves by an abbreviation the
/// index spells out: the maker, the beginnings of the words that mark the
/// line ("qc" for "QC45", "QC Ultra"), and what to search for. A line and
/// not a maker, since a maker's speakers and soundbars carry the maker's
/// name too.
const ABBREVIATED: &[(&str, &[&str], &str)] = &[
    ("bose", &["qc", "quietcomfort"], "Bose QuietComfort"),
    ("bose", &["nc", "700"], "Bose Noise Cancelling"),
];

/// Makers whose names are more than one word, as the index spells them, and
/// that neither rule in `maker_of` finds.
const MULTIWORD_MAKERS: &[&str] = &[
    "Alpha Omega",
    "Audio Genetic",
    "Audio Zenith",
    "Custom Art",
    "Dan Clark Audio",
    "NF ACOUS",
    "Queen of Audio",
    "Sound Rhyme",
    "Tansio Mirai",
    "Turtle Beach",
    "Unique Melody",
];

/// Second words that make a maker's name two words: "Final Audio", "Kiwi
/// Ears", "LZ Hi-Fi".
const MAKER_SUFFIXES: &[&str] = &[
    "Acoustics",
    "Audio",
    "Ears",
    "Electronics",
    "Hi-Fi",
    "HiFi",
    "Lab",
    "Technology",
];

/// The maker an entry's name begins with: one listed in `MULTIWORD_MAKERS`;
/// otherwise three words around an ampersand ("Bowers & Wilkins"), two where
/// the second is a company word ("Simgot Audio"), or else the first word.
pub fn maker_of(name: &str) -> &str {
    let listed = MULTIWORD_MAKERS.iter().find(|m| {
        name.strip_prefix(**m)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(' '))
    });
    if let Some(m) = listed {
        return m;
    }
    let words: Vec<&str> = name.split_whitespace().collect();
    let take = match words.as_slice() {
        [_, "&", _, ..] => 3,
        [_, second, ..] if MAKER_SUFFIXES.contains(second) => 2,
        _ => 1,
    };
    // The name's own text, up to the end of the last word taken.
    let end = name
        .split_whitespace()
        .take(take)
        .last()
        .map(|w| w.as_ptr() as usize - name.as_ptr() as usize + w.len())
        .unwrap_or(name.len());
    &name[..end]
}

/// The makers in the index, each with how many results it has, in
/// alphabetical order: what Find in AutoEQ lists before anything is typed.
pub fn makers(entries: &[Entry]) -> Vec<(String, usize)> {
    let mut counts: std::collections::BTreeMap<String, (String, usize)> = Default::default();
    for e in entries {
        let maker = maker_of(&e.name);
        let slot = counts
            .entry(maker.to_lowercase())
            .or_insert_with(|| (maker.to_owned(), 0));
        slot.1 += 1;
    }
    counts.into_values().collect()
}

/// `maker`'s results, by model name, and within one model in the index's
/// order, which puts AutoEQ's preferred source first.
pub fn models<'a>(entries: &'a [Entry], maker: &str) -> Vec<&'a Entry> {
    let mut found: Vec<&Entry> = entries
        .iter()
        .filter(|e| maker_of(&e.name).eq_ignore_ascii_case(maker))
        .collect();
    found.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(a.number.cmp(&b.number))
    });
    found
}

/// The AutoEQ entry to offer for `device`: none once it has a profile, or
/// once its suggestion was turned down.
/// What to offer an output about AutoEQ: its profile, when its name says
/// which it is, or a search to pick one from, when it says only roughly.
#[derive(Debug, Clone, PartialEq)]
pub enum Offer {
    Profile(Entry),
    Search(String),
}

/// What to offer `device`: nothing once it has a profile, or once the offer
/// was turned down.
pub fn suggestion(device: &str) -> Result<Option<Offer>, String> {
    let dsp = &Config::cached().dsp;
    if dsp.autoeq_dismissed.iter().any(|d| d == device)
        || dsp
            .profiles
            .iter()
            .any(|p| p.devices.iter().any(|d| d == device))
    {
        return Ok(None);
    }
    let entries = index(Freshness::Daily)?;
    Ok(match suggest(&entries, device) {
        Some(e) => Some(Offer::Profile(e.clone())),
        None => search_for(&entries, device).map(Offer::Search),
    })
}

/// Stop offering AutoEQ's profile for `device`. Kept with the machine's
/// devices, in `config.local.toml`.
pub fn dismiss(device: &str) -> Result<(), String> {
    Config::persist(|cfg| {
        if !cfg.dsp.autoeq_dismissed.iter().any(|d| d == device) {
            cfg.dsp.autoeq_dismissed.push(device.to_owned());
        }
    })
    .map_err(|e| e.to_string())
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
    let name = profiles::save(imported(entry, &text)?, None)?;
    // The result's measurement and target, for moving it to another target
    // and drawing it. Without them the correction still plays as made.
    match keep_result(entry, &name) {
        Ok(made_for) => profiles::set_made_for(&name, made_for)?,
        Err(e) => log::info!("autoeq: {}: no measurement kept: {e}", entry.name),
    }
    Ok(name)
}

/// Fetch `entry`'s result CSV and keep it beside the profile `name`. Answers
/// with the shipped target it was made for, when it is one.
fn keep_result(entry: &Entry, name: &str) -> Result<Option<&'static str>, String> {
    let url = entry.csv_url().ok_or("no address for the result")?;
    let resp = http()?
        .get(url)
        .send()
        .map_err(|e| e.without_url().to_string())?;
    if !resp.status().is_success() {
        return Err(format!("GitHub answered {}", resp.status()));
    }
    let text = body(resp, RESULT_CAP)?;
    let target = super::targets::result_column(&text, "target");
    if target.is_empty() {
        return Err("the result has no target column".into());
    }
    let dir = profiles::dir(name);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(super::targets::result_path(&dir), &text).map_err(|e| e.to_string())?;
    Ok(super::targets::identify(&target).map(|t| t.id))
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

    /// A name that says roughly which headphone, and nothing more exact,
    /// opens a search for the model, or failing that the maker.
    #[test]
    fn a_rough_name_gets_a_search_for_its_model() {
        let entries = parse_index(
            "- [Apple AirPods (2nd generation)](./a/in-ear/Apple%20AirPods%20(2nd%20generation)) by a
- [Apple AirPods Pro](./a/in-ear/Apple%20AirPods%20Pro) by a
- [Apple AirPods Pro 2 (ANC mode)](./a/in-ear/Apple%20AirPods%20Pro%202%20(ANC%20mode)) by a
- [Apple AirPods 4 (ANC on)](./a/in-ear/Apple%20AirPods%204%20(ANC%20on)) by a
- [Bose QuietComfort 45](./b/over-ear/Bose%20QuietComfort%2045) by b
- [Brainwavz M2](./d/in-ear/Brainwavz%20M2) by d
- [LG Tone Free](./d/in-ear/LG%20Tone%20Free) by d
- [FiiO FH5](./d/in-ear/FiiO%20FH5) by d
",
        );
        let search = |device: &str| search_for(&entries, device);
        assert_eq!(search("Jo's AirPods Pro").as_deref(), Some("AirPods Pro"));
        assert_eq!(search("AirPods").as_deref(), Some("AirPods"));
        assert_eq!(search("Jo’s AirPods 4").as_deref(), Some("AirPods 4"));
        assert_eq!(search("Bose QC45").as_deref(), Some("Bose QuietComfort"));
        assert_eq!(
            search("Bose QC Ultra Headphones").as_deref(),
            Some("Bose QuietComfort")
        );
        assert_eq!(
            search("Bose NC 700").as_deref(),
            Some("Bose Noise Cancelling")
        );
        assert_eq!(search("Bose SoundLink Flex"), None, "a speaker");
        assert_eq!(search("Bose Color II SoundLink"), None, "a speaker");
        assert_eq!(search("Bose Smart Soundbar 600"), None);
        assert_eq!(search("MOTU M2"), None, "a short model is not enough");
        assert_eq!(search("Scarlett 4i4 USB"), None);
        assert_eq!(search("LG UltraFine Display Audio"), None);
        assert_eq!(search("MacBook Pro Speakers"), None);
        assert_eq!(search("FiiO K3"), None, "a maker of DACs too is no search");
        assert_eq!(
            search("Focusrite Scarlett Solo"),
            None,
            "a short word alone is no search"
        );
    }

    #[test]
    fn makers_and_their_models_come_from_the_names() {
        assert_eq!(maker_of("Sennheiser HD 650"), "Sennheiser");
        assert_eq!(maker_of("64 Audio A12t"), "64 Audio");
        assert_eq!(maker_of("Bang & Olufsen Beoplay H9"), "Bang & Olufsen");
        assert_eq!(maker_of("Final Audio E3000"), "Final Audio");
        assert_eq!(maker_of("Bowers & Wilkins PX7"), "Bowers & Wilkins");
        assert_eq!(maker_of("Simgot Audio EA500"), "Simgot Audio");
        assert_eq!(maker_of("Kiwi Ears Orchestra Lite"), "Kiwi Ears");
        assert_eq!(maker_of("LZ Hi-Fi A7"), "LZ Hi-Fi");
        assert_eq!(maker_of("Dan Clark Audio Aeon 2 Closed"), "Dan Clark Audio");
        assert_eq!(maker_of("Audio Zenith PMx2"), "Audio Zenith");
        assert_eq!(maker_of("Audio-Technica ATH-M50x"), "Audio-Technica");
        assert_eq!(maker_of("Sennheiser HD 650"), "Sennheiser");
        assert_eq!(
            maker_of("Finalist X"),
            "Finalist",
            "a prefix, not a word, is not a maker"
        );
        let entries = parse_index(INDEX);
        let makers = makers(&entries);
        assert_eq!(
            makers,
            vec![("1MORE".to_owned(), 1), ("Sennheiser".to_owned(), 5)]
        );
        let hd: Vec<(&str, &str)> = models(&entries, "sennheiser")
            .iter()
            .map(|e| (e.name.as_str(), e.source.as_str()))
            .collect();
        assert_eq!(
            hd,
            [
                ("Sennheiser HD 600", "oratory1990"),
                ("Sennheiser HD 650", "oratory1990"),
                ("Sennheiser HD 650", "crinacle"),
                ("Sennheiser HD 650 (2020)", "Innerfidelity"),
                ("Sennheiser HD 660 S", "oratory1990"),
            ]
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

    /// Output names as macOS and iOS give them: interfaces, DACs, built-in
    /// speakers, Bluetooth headphones by the name they ship with or were
    /// given. None should be offered a profile, against the real index too
    /// (`suggestions_against_the_live_index`).
    const NOT_HEADPHONES: &[&str] = &[
        "MacBook Pro Speakers",
        "MacBook Air Speakers",
        "Mac mini Speakers",
        "Mac Studio Speakers",
        "iMac Speakers",
        "Studio Display Speakers",
        "LG UltraFine Display Audio",
        "External Headphones",
        "Headphones",
        "Speaker",
        "Speaker 2",
        "Built-in Output",
        "BlackHole 2ch",
        "Microsoft Teams Audio",
        "ZoomAudioDevice",
        "Scarlett 2i2 USB",
        "Scarlett 4i4 USB",
        "Focusrite Scarlett Solo",
        "Volt 2",
        "Universal Audio Volt 276",
        "MOTU M2",
        "MOTU M4",
        "Arturia MiniFuse 2",
        "Apogee Duet",
        "RME ADI-2 DAC",
        "Zoom H6",
        "Mojo 2",
        "Chord Hugo 2",
        "Hugo 2",
        "Topping E30",
        "Topping DX3 Pro+",
        "SMSL SU-1",
        "SMSL M200",
        "FiiO K3",
        "FiiO K5 Pro",
        "iFi ZEN DAC V2",
        "Zen Air DAC",
        "Schiit Modi",
        "AirPods",
        "AirPods 4",
        "AirPods Pro",
        "AirPods Pro 3",
        "Jo's AirPods Pro",
        "Marshall Major IV",
        "Galaxy Buds+",
        "CMF Buds Pro 2",
    ];

    #[test]
    fn a_device_is_suggested_a_profile_only_when_its_name_says_which() {
        let entries = parse_index(
            "- [Apple AirPods](./a/in-ear/Apple%20AirPods) by a
- [Apple AirPods Pro](./a/in-ear/Apple%20AirPods%20Pro) by a
- [Apple AirPods Pro 2](./a/in-ear/Apple%20AirPods%20Pro%202) by a
- [Apple AirPods Max](./a/over-ear/Apple%20AirPods%20Max) by a
- [Sony WH-1000XM4](./b/over-ear/Sony%20WH-1000XM4) by b
- [Sony WH-1000XM4](./c/over-ear/Sony%20WH-1000XM4) by c
- [Sony WH-1000XM4 (ANC off)](./b/over-ear/Sony%20WH-1000XM4%20(ANC%20off)) by b
- [Ortofon 2](./d/in-ear/Ortofon%202) by d
- [Ortofon 1](./d/in-ear/Ortofon%201) by d
- [Brainwavz M2](./d/in-ear/Brainwavz%20M2) by d
- [Advanced M4](./d/in-ear/Advanced%20M4) by d
- [Magaosi K3](./d/in-ear/Magaosi%20K3) by d
- [EPZ K5](./d/in-ear/EPZ%20K5) by d
- [Somic V2](./d/over-ear/Somic%20V2) by d
- [Tingker H6](./d/in-ear/Tingker%20H6) by d
- [Creative Zen Air](./d/in-ear/Creative%20Zen%20Air) by d
- [KEF M200](./d/in-ear/KEF%20M200) by d
- [Marshall Major](./d/on-ear/Marshall%20Major) by d
- [Beats Studio Buds](./d/in-ear/Beats%20Studio%20Buds) by d
- [Samsung Galaxy Buds](./d/in-ear/Samsung%20Galaxy%20Buds) by d
- [Samsung Galaxy Buds+](./d/in-ear/Samsung%20Galaxy%20Buds+) by d
- [OnePlus Buds Pro 2](./d/in-ear/OnePlus%20Buds%20Pro%202) by d
- [Sennheiser HD 600](./e/over-ear/Sennheiser%20HD%20600) by e
",
        );
        let named =
            |device: &str| suggest(&entries, device).map(|e| (e.name.as_str(), e.source.as_str()));
        assert_eq!(named("Sony WH-1000XM4"), Some(("Sony WH-1000XM4", "b")));
        assert_eq!(
            named("Jo's Sony WH-1000XM4"),
            Some(("Sony WH-1000XM4", "b"))
        );
        assert_eq!(
            named("Apple AirPods Pro 2"),
            Some(("Apple AirPods Pro 2", "a"))
        );
        assert_eq!(
            named("Samsung Galaxy Buds+"),
            Some(("Samsung Galaxy Buds+", "d"))
        );
        assert_eq!(named("Sennheiser HD 600"), Some(("Sennheiser HD 600", "e")));
        assert_eq!(
            named("Apple AirPods Pro 3"),
            None,
            "a generation the index lacks"
        );
        // Models that say which they are without the maker.
        assert_eq!(named("WH-1000XM4"), Some(("Sony WH-1000XM4", "b")));
        assert_eq!(named("Jo's AirPods Max"), Some(("Apple AirPods Max", "a")));
        // Not on the list: AutoEQ has only its modes, never the plain model.
        assert_eq!(named("AirPods Pro 2"), None);
        // Every generation calls itself these, and both AirPods 4 models
        // the one name.
        assert_eq!(named("AirPods 4"), None);
        assert_eq!(named("Jo's AirPods Pro"), None);
        assert_eq!(named("AirPods"), None);
        assert_eq!(named("AirPods Pro 3"), None);
        assert_eq!(named("WH-1000XM4 (ANC off)"), None, "a variant, maker-less");
        assert_eq!(named("Samsung Galaxy Buds2"), None);
        for device in NOT_HEADPHONES {
            assert_eq!(named(device), None, "{device}");
        }
    }

    /// The rule against AutoEQ's real index, kept out of the default run
    /// because it needs the file: download `results/INDEX.md`, point
    /// `KOAN_AUTOEQ_INDEX` at it and run the ignored tests.
    #[test]
    #[ignore]
    fn suggestions_against_the_live_index() {
        let path = std::env::var("KOAN_AUTOEQ_INDEX").expect("KOAN_AUTOEQ_INDEX");
        let entries = parse_index(&std::fs::read_to_string(path).unwrap());
        assert!(entries.len() > 1000);
        let wrong: Vec<String> = NOT_HEADPHONES
            .iter()
            .filter_map(|d| suggest(&entries, d).map(|e| format!("{d} → {}", e.name)))
            .collect();
        assert!(wrong.is_empty(), "{wrong:#?}");
        let found = |d: &str| suggest(&entries, d).map(|e| e.name.clone());
        assert_eq!(found("Sony WH-1000XM4").as_deref(), Some("Sony WH-1000XM4"));
        assert_eq!(
            found("Sennheiser HD 650").as_deref(),
            Some("Sennheiser HD 650")
        );
        // Maker-less names that say which headphone they are.
        for (device, entry) in [
            ("WH-1000XM4", "Sony WH-1000XM4"),
            ("WF-1000XM5", "Sony WF-1000XM5"),
            ("Jo's AirPods Max", "Apple AirPods Max"),
            ("Galaxy Buds2 Pro", "Samsung Galaxy Buds2 Pro"),
        ] {
            assert_eq!(found(device).as_deref(), Some(entry), "{device}");
        }
        // Outputs that are not headphones get no search either.
        let searched: Vec<String> = [
            "MacBook Pro Speakers",
            "Studio Display Speakers",
            "LG UltraFine Display Audio",
            "Built-in Output",
            "BlackHole 2ch",
            "Scarlett 2i2 USB",
            "Focusrite Scarlett Solo",
            "Volt 2",
            "Universal Audio Volt 276",
            "MOTU M2",
            "MOTU M4",
            "Arturia MiniFuse 2",
            "Apogee Duet",
            "RME ADI-2 DAC",
            "Zoom H6",
            "Mojo 2",
            "Chord Hugo 2",
            "Hugo 2",
            "Topping E30",
            "Topping DX3 Pro+",
            "SMSL SU-1",
            "SMSL M200",
            "FiiO K3",
            "FiiO K5 Pro",
            "iFi ZEN DAC V2",
            "Zen Air DAC",
            "Schiit Modi",
            "External Headphones",
            "Headphones",
            "Bose SoundLink Flex",
            "Bose SoundLink Micro",
            "Bose SoundLink Revolve+ II",
            "Bose Solo 5",
            "Bose Smart Soundbar 600",
            "Bose Color II SoundLink",
            "Bose Revolve+ II SoundLink",
            "Bose Micro SoundLink",
        ]
        .iter()
        .filter_map(|d| search_for(&entries, d).map(|q| format!("{d} → {q}")))
        .collect();
        assert!(searched.is_empty(), "{searched:#?}");
        // Rough names get a search for their model.
        assert_eq!(
            search_for(&entries, "Jo's AirPods Pro").as_deref(),
            Some("AirPods Pro")
        );
        assert_eq!(
            search_for(&entries, "Bose QC45").as_deref(),
            Some("Bose QuietComfort")
        );
        // Every entry in the curated list is one the index has.
        for name in MAKERLESS {
            assert!(
                entries.iter().any(|e| e.name == *name),
                "{name} is not in the index"
            );
        }
        // Headphones that ship named in full, maker first.
        for (device, entry) in [
            ("Beats Studio Buds +", "Beats Studio Buds +"),
            ("Nothing Ear (2)", "Nothing ear (2)"),
            ("Jabra Elite 85t", "Jabra Elite 85t"),
        ] {
            assert_eq!(found(device).as_deref(), Some(entry), "{device}");
        }
    }
}
