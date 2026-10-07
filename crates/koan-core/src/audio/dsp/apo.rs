//! Equalizer APO's `config.txt`, which is also what AutoEQ's
//! `ParametricEQ.txt` and REW's filter exports are written in. See
//! <https://sourceforge.net/p/equalizerapo/wiki/Configuration%20reference/>.
//!
//! ```text
//! Preamp: -6.8 dB
//! Filter 1: ON PK Fc 20 Hz Gain -1.3 dB Q 2.000
//! Channel: R
//! Filter: ON LSC 6 dB Fc 105 Hz Gain 5.5 dB
//! Delay: 0.3 ms
//! Copy: L=0.9*L+0.1*R R=0.1*L+0.9*R
//! Convolution: room-48k.wav
//! ```
//!
//! What has no equivalent in koan is refused by name rather than skipped:
//! dropping a band, or a delay, changes the correction without saying so.
//! Responses run after every other step, so a `Copy:` after a `Convolution:`
//! is refused too; bands and delays act per channel and commute with them.

use std::collections::BTreeMap;
use std::path::Path;

use super::impulse::{Impulse, Route, read_audio};
use crate::config::{Delay, DspFilter, EqFilter, EqFilterKind, GraphicEq, Mix};

#[derive(Debug, Default, PartialEq)]
pub struct Parsed {
    /// What `Preamp:` asked for across every channel.
    pub preamp_db: Option<f64>,
    pub filters: Vec<DspFilter>,
    pub impulses: Vec<Impulse>,
}

/// At one rate: a file for every channel, and responses for single channels.
type AtRate = (Option<Vec<Vec<f32>>>, BTreeMap<u16, Vec<f32>>);

/// Responses gathered by rate.
#[derive(Default)]
pub(super) struct Convolutions(BTreeMap<u32, AtRate>);

pub fn read(path: &Path) -> Result<Parsed, String> {
    let mut parsed = Parsed::default();
    let mut convolutions = Convolutions::default();
    read_into(path, &mut parsed, &mut convolutions, 0)?;
    parsed.impulses = convolutions.build();
    common_preamp(&mut parsed);
    Ok(parsed)
}

/// Text without a file behind it — pasted, or shared from a chat. A
/// `Convolution:` or `Include:` in it has nothing to be relative to.
pub fn parse(text: &str) -> Result<Parsed, String> {
    let mut parsed = Parsed::default();
    let mut convolutions = Convolutions::default();
    parse_into(text, None, &mut parsed, &mut convolutions, 0)?;
    parsed.impulses = convolutions.build();
    common_preamp(&mut parsed);
    Ok(parsed)
}

/// A preamp given per channel that comes to the same on both sides of a
/// stereo pair and on every other channel the file names, as squig.link's
/// two-channel export writes it, is the profile's preamp rather than a band
/// on each channel. Preamps that differ stay as gain bands on their channels,
/// as they do in a file with a `Copy`, whose mix a gain ahead of it changes.
fn common_preamp(parsed: &mut Parsed) {
    if parsed
        .filters
        .iter()
        .any(|f| matches!(f, DspFilter::Mix(_)))
    {
        return;
    }
    let preamp = |f: &DspFilter| matches!(f, DspFilter::Band(b) if b.kind == EqFilterKind::Gain);
    let mut named: BTreeMap<u16, f64> = BTreeMap::new();
    for f in &parsed.filters {
        for &c in f.channels() {
            named.entry(c).or_insert(0.0);
        }
    }
    for f in &parsed.filters {
        if let DspFilter::Band(b) = f
            && preamp(f)
        {
            for c in &b.channels {
                *named.entry(*c).or_insert(0.0) += b.gain_db;
            }
        }
    }
    let Some(&first) = named.values().next() else {
        return;
    };
    let stereo = named.contains_key(&0) && named.contains_key(&1);
    if first == 0.0 || !stereo || named.values().any(|db| (db - first).abs() > 1e-9) {
        return;
    }
    parsed.filters.retain(|f| !preamp(f));
    *parsed.preamp_db.get_or_insert(0.0) += first;
}

/// Whether `text` reads as Equalizer APO configuration.
pub fn looks_like(text: &str) -> bool {
    text.lines().map(str::trim).any(|l| {
        (l.starts_with("Filter") && l.contains(':'))
            || [
                "Preamp:",
                "Convolution:",
                "Include:",
                "GraphicEQ:",
                "Copy:",
                "Delay:",
            ]
            .iter()
            .any(|c| l.starts_with(c))
    })
}

fn read_into(
    path: &Path,
    parsed: &mut Parsed,
    convolutions: &mut Convolutions,
    depth: usize,
) -> Result<(), String> {
    if depth > 8 {
        return Err("includes nested too deeply".into());
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse_into(&text, path.parent(), parsed, convolutions, depth)
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn parse_into(
    text: &str,
    dir: Option<&Path>,
    parsed: &mut Parsed,
    convolutions: &mut Convolutions,
    depth: usize,
) -> Result<(), String> {
    // Channels the following commands apply to; `None` is all of them.
    let mut selection: Option<Vec<u16>> = None;
    // Inside an `If:`: a sample-rate condition, which only a response can
    // honour, since a response already belongs to one rate.
    let mut conditional = 0usize;
    // REW's "Filter Settings file" opens with lines of its own — the version,
    // `Dated:`, `Notes:`, `Equaliser:` — which say nothing about the sound.
    let rew = text
        .lines()
        .map(|l| l.trim().trim_start_matches('\u{feff}'))
        .find(|l| !l.is_empty())
        .is_some_and(|l| l.starts_with("Filter Settings file"));

    for (n, line) in text.lines().enumerate() {
        let line = line.trim().trim_start_matches('\u{feff}');
        // `//` opens a comment in Qudelix's exports, as `#` does in APO's.
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            continue;
        }
        let is_filter = line.split_once(':').is_some_and(|(c, _)| {
            c.trim()
                .strip_prefix("Filter")
                .is_some_and(|n| n.trim().chars().all(|c| c.is_ascii_digit()))
        });
        if rew && !is_filter {
            continue;
        }
        let fail = |why: &str| format!("line {}: {why}: {line}", n + 1);
        let Some((command, rest)) = line.split_once(':') else {
            return Err(fail("not a command"));
        };
        let rest = rest.trim();
        let command = command.trim();
        let file = |name: &str| -> Result<std::path::PathBuf, String> {
            let dir =
                dir.ok_or_else(|| fail("names a file, and this text has no folder to find it in"))?;
            Ok(dir.join(name.replace('\\', "/")))
        };
        let word = command.split_whitespace().next().unwrap_or("");
        if conditional > 0 && matches!(word, "Filter" | "Delay" | "Copy" | "GraphicEQ") {
            return Err(fail(&format!(
                "{word} that depends on the sample rate is not supported"
            )));
        }
        match word {
            "Preamp" => {
                if conditional > 0 {
                    return Err(fail(
                        "a preamp that depends on the sample rate is not supported",
                    ));
                }
                let db = rest
                    .trim_end_matches("dB")
                    .trim()
                    .parse::<f64>()
                    .map_err(|_| fail("bad preamp"))?;
                match &selection {
                    None => *parsed.preamp_db.get_or_insert(0.0) += db,
                    Some(channels) => parsed.filters.push(DspFilter::Band(EqFilter {
                        kind: EqFilterKind::Gain,
                        freq: 1000.0,
                        gain_db: db,
                        q: 1.0,
                        channels: channels.clone(),
                    })),
                }
            }
            "Filter" => {
                if let Some(mut filter) = filter(rest).map_err(|e| fail(&e))? {
                    filter.channels = selection.clone().unwrap_or_default();
                    parsed.filters.push(filter.into());
                }
            }
            "Delay" => {
                let mut delay = delay(rest).map_err(|e| fail(&e))?;
                delay.channels = selection.clone().unwrap_or_default();
                parsed.filters.push(DspFilter::Delay(delay));
            }
            "Copy" => {
                // Shared across `Include:`, so a response in any file counts.
                if convolutions.holds_any() {
                    return Err(fail(
                        "a Copy after a Convolution is not supported: responses run last",
                    ));
                }
                parsed
                    .filters
                    .push(DspFilter::Mix(copy(rest).map_err(|e| fail(&e))?));
            }
            "GraphicEQ" => {
                let mut graphic = graphic(rest).map_err(|e| fail(&e))?;
                graphic.channels = selection.clone().unwrap_or_default();
                parsed.filters.push(DspFilter::Graphic(graphic));
            }
            "Channel" => selection = channels(rest).map_err(|e| fail(&e))?,
            "Convolution" => {
                let path = file(rest)?;
                let (rate, chans) =
                    read_audio(&path).map_err(|e| fail(&format!("{}: {e}", path.display())))?;
                convolutions.add(rate, chans, selection.as_deref());
            }
            "Include" => read_into(&file(rest)?, parsed, convolutions, depth + 1)?,
            "If" => conditional += 1,
            "ElseIf" | "Else" => {}
            "EndIf" => conditional = conditional.saturating_sub(1),
            // Which device the settings are for is koan's to decide.
            "Device" => {}
            // Qudelix's export: what kind of preset it is, ahead of the
            // filters, and the headphone's impedance and sensitivity after.
            "TYPE" if rest.eq_ignore_ascii_case("PEQ") => {}
            "TYPE" => {
                return Err(fail(&format!(
                    "a {rest} preset is not supported, only a PEQ one"
                )));
            }
            "IMPEDANCE" | "SENSITIVITY" => {}
            other => return Err(fail(&format!("{other} is not supported"))),
        }
    }
    Ok(())
}

impl Convolutions {
    pub(super) fn holds_any(&self) -> bool {
        !self.0.is_empty()
    }

    pub(super) fn add(&mut self, rate: u32, chans: Vec<Vec<f32>>, selection: Option<&[u16]>) {
        let (all, per) = self.0.entry(rate).or_default();
        match selection {
            None => *all = Some(chans),
            // Round-robin, as Equalizer APO assigns a file's channels.
            Some(selected) => {
                for (k, &c) in selected.iter().enumerate() {
                    per.insert(c, chans[k % chans.len()].clone());
                }
            }
        }
    }

    /// One response per rate. Channels a per-channel response leaves out pass
    /// through, as they do in Equalizer APO.
    pub(super) fn build(self) -> Vec<Impulse> {
        self.0
            .into_iter()
            .map(|(rate, (all, per))| {
                if per.is_empty() {
                    return Impulse::from_channels(rate, all.unwrap_or_default());
                }
                let n = per.keys().max().map_or(2, |&m| (m as usize + 1).max(2));
                let routes = (0..n)
                    .map(|c| Route {
                        ir: per.get(&(c as u16)).cloned().unwrap_or_else(|| vec![1.0]),
                        inputs: vec![(c, 1.0)],
                        outputs: vec![(c, 1.0)],
                    })
                    .collect();
                Impulse {
                    rate,
                    channels: Some(n),
                    routes,
                    in_delays: Vec::new(),
                    out_delays: Vec::new(),
                }
            })
            .collect()
    }
}

/// `10 ms`, `441 samples`.
fn delay(body: &str) -> Result<Delay, String> {
    let split = body
        .find(|c: char| c.is_ascii_alphabetic())
        .ok_or("expected ms or samples")?;
    let value: f64 = body[..split]
        .trim()
        .parse()
        .map_err(|_| "bad delay".to_string())?;
    match body[split..].trim() {
        "ms" => Ok(Delay {
            ms: value,
            ..Default::default()
        }),
        "samples" => Ok(Delay {
            samples: value,
            ..Default::default()
        }),
        unit => Err(format!("{unit} is not a delay unit")),
    }
}

/// `L=0.5*L+0.5*R R=L C=-6dB*L+-6dB*R`. Every source is read before any
/// target is written; channels no assignment names keep what they had.
fn copy(body: &str) -> Result<Mix, String> {
    // Spaces around operators, and before `dB`, belong to the term beside them.
    let mut joined = String::new();
    let mut chars = body.trim().chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_whitespace() {
            while chars.peek().is_some_and(|c| c.is_whitespace()) {
                chars.next();
            }
            let next_joins = chars.peek().is_some_and(|&n| "=+*".contains(n) || n == 'd');
            if !next_joins && !joined.ends_with(['=', '+', '*']) {
                joined.push(' ');
            }
        } else {
            joined.push(c);
        }
    }
    let mut targets: BTreeMap<u16, Vec<(u16, f64)>> = BTreeMap::new();
    for assignment in joined.split_whitespace() {
        let (target, expr) = assignment
            .split_once('=')
            .ok_or_else(|| format!("expected target=sources, got {assignment}"))?;
        let mut sources = Vec::new();
        for term in expr.split('+').filter(|t| !t.is_empty()) {
            let (factor, channel) = match term.split_once('*') {
                Some((f, c)) => (factor(f)?, c),
                None => match term.strip_prefix('-') {
                    Some(c) => (-1.0, c),
                    None => (1.0, term),
                },
            };
            if let Ok(constant) = channel.parse::<f64>() {
                if constant == 0.0 {
                    continue;
                }
                return Err(format!("a constant ({term}) is not supported"));
            }
            sources.push((one_channel(channel)?, factor));
        }
        targets.insert(one_channel(target)?, sources);
    }
    let n = targets.keys().max().map_or(0, |&m| m as usize + 1);
    Ok(Mix {
        outputs: (0..n as u16)
            .map(|c| targets.remove(&c).unwrap_or_else(|| vec![(c, 1.0)]))
            .collect(),
    })
}

/// `0.5` or `-6dB`.
fn factor(text: &str) -> Result<f64, String> {
    let bad = || format!("bad factor {text}");
    match text.strip_suffix("dB") {
        Some(db) => Ok(10f64.powf(db.trim().parse::<f64>().map_err(|_| bad())? / 20.0)),
        None => text.parse().map_err(|_| bad()),
    }
}

fn one_channel(name: &str) -> Result<u16, String> {
    match channels(name)? {
        Some(list) if list.len() == 1 => Ok(list[0]),
        _ => Err(format!("{name} is not one channel")),
    }
}

/// `20 -1.5; 21 -1.4; …`: Hz and dB pairs.
fn graphic(body: &str) -> Result<GraphicEq, String> {
    let mut points = Vec::new();
    for pair in body.split(';').map(str::trim).filter(|p| !p.is_empty()) {
        let mut it = pair.split_whitespace().map(str::parse::<f64>);
        match (it.next(), it.next(), it.next()) {
            (Some(Ok(hz)), Some(Ok(db)), None) => points.push((hz, db)),
            _ => return Err(format!("bad point {pair}")),
        }
    }
    if points.is_empty() {
        return Err("no points".into());
    }
    Ok(GraphicEq {
        points,
        channels: Vec::new(),
    })
}

fn channels(spec: &str) -> Result<Option<Vec<u16>>, String> {
    let mut out = Vec::new();
    for t in spec.split_whitespace() {
        let c = match t.to_ascii_uppercase().as_str() {
            "ALL" => return Ok(None),
            "L" => 0,
            "R" => 1,
            "C" => 2,
            "LFE" => 3,
            "RL" => 4,
            "RR" => 5,
            "SL" => 6,
            "SR" => 7,
            n => match n.parse::<u16>() {
                Ok(i) if i >= 1 => i - 1,
                _ => return Err(format!("unknown channel {t}")),
            },
        };
        out.push(c);
    }
    Ok(Some(out))
}

/// One `Filter:` line's body. `None` for one switched off.
fn filter(body: &str) -> Result<Option<EqFilter>, String> {
    let words: Vec<&str> = body.split_whitespace().collect();
    match words.first() {
        Some(&"ON") => {}
        Some(&"OFF") => return Ok(None),
        _ => return Err("expected ON or OFF".into()),
    }
    let kind_word = words.get(1).copied().unwrap_or("");
    // REW writes every slot, the unused ones as `ON None`.
    if kind_word == "None" {
        return Ok(None);
    }
    // `LSC 6 dB`: a slope in dB per octave right after the type.
    let slope = match (words.get(2), words.get(3)) {
        (Some(v), Some(&"dB")) if matches!(kind_word, "LSC" | "HSC") => v.parse::<f64>().ok(),
        _ => None,
    };
    let kind = match kind_word {
        "PK" | "PEQ" | "Modal" => EqFilterKind::Peaking,
        "LS" | "LSC" => EqFilterKind::LowShelf,
        "HS" | "HSC" => EqFilterKind::HighShelf,
        "LP" | "LPQ" => EqFilterKind::LowPass,
        "HP" | "HPQ" => EqFilterKind::HighPass,
        "BP" => EqFilterKind::BandPass,
        "NO" => EqFilterKind::Notch,
        "AP" => EqFilterKind::AllPass,
        other => return Err(format!("{other} filters are not supported")),
    };
    // `LS 12dB` and `HS 12dB` are second-order shelves at the corner; the
    // 6 dB ones are first-order.
    let first_order = matches!(kind_word, "LS" | "HS")
        && (words.get(2) == Some(&"6dB")
            || (words.get(2) == Some(&"6") && words.get(3) == Some(&"dB")));
    let kind = match kind {
        EqFilterKind::LowShelf if first_order => EqFilterKind::LowShelfFirstOrder,
        EqFilterKind::HighShelf if first_order => EqFilterKind::HighShelfFirstOrder,
        k => k,
    };
    let value = |key: &str| -> Result<Option<f64>, String> {
        match words.iter().position(|w| *w == key) {
            Some(i) => {
                let at = if key == "BW" && words.get(i + 1) == Some(&"Oct") {
                    i + 2
                } else {
                    i + 1
                };
                words
                    .get(at)
                    .and_then(|v| v.parse().ok())
                    .map(Some)
                    .ok_or_else(|| format!("bad {key}"))
            }
            None => Ok(None),
        }
    };
    let freq = value("Fc")?.ok_or("no Fc")?;
    let gain_db = value("Gain")?.unwrap_or(0.0);
    let q = match (value("Q")?, value("BW")?, slope) {
        (Some(q), _, _) => q,
        (None, Some(bw), _) => q_from_bandwidth(bw),
        (None, None, Some(s)) => q_from_slope(gain_db, s),
        _ => std::f64::consts::FRAC_1_SQRT_2,
    };
    Ok(Some(EqFilter {
        kind,
        freq,
        gain_db,
        q,
        channels: Vec::new(),
    }))
}

/// RBJ's conversion from bandwidth in octaves.
pub(crate) fn q_from_bandwidth(octaves: f64) -> f64 {
    1.0 / (2.0 * (std::f64::consts::LN_2 / 2.0 * octaves).sinh())
}

/// RBJ's shelf slope, where 12 dB per octave is S = 1.
pub(crate) fn q_from_slope(gain_db: f64, db_per_octave: f64) -> f64 {
    let a = 10f64.powf(gain_db / 40.0);
    let s = (db_per_octave / 12.0).clamp(1e-3, 1.0);
    1.0 / ((a + 1.0 / a) * (1.0 / s - 1.0) + 2.0).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::dsp::raw::write_wav;

    fn band(kind: EqFilterKind, freq: f64, gain_db: f64, q: f64) -> EqFilter {
        EqFilter {
            kind,
            freq,
            gain_db,
            q,
            channels: Vec::new(),
        }
    }

    #[test]
    fn reads_an_autoeq_file() {
        let text = "Preamp: -6.8 dB\n\
            Filter 1: ON PK Fc 20 Hz Gain -1.3 dB Q 2.000\n\
            Filter 2: ON LSC Fc 105 Hz Gain 5.5 dB Q 0.70\n\
            Filter 3: OFF PK Fc 36 Hz Gain 0.7 dB Q 2.000\n\
            Filter 4: ON HS Fc 10000 Hz Gain 2.5 dB\n";
        let parsed = parse(text).unwrap();
        assert_eq!(parsed.preamp_db, Some(-6.8));
        assert_eq!(
            parsed.filters,
            [
                band(EqFilterKind::Peaking, 20.0, -1.3, 2.0),
                band(EqFilterKind::LowShelf, 105.0, 5.5, 0.7),
                band(
                    EqFilterKind::HighShelf,
                    10000.0,
                    2.5,
                    std::f64::consts::FRAC_1_SQRT_2
                ),
            ]
            .map(DspFilter::from)
        );
    }

    #[test]
    fn channel_sections_and_slopes() {
        let text = "Channel: L\nFilter: ON PK Fc 100 Hz Gain -3 dB BW Oct 1.0\n\
            Channel: 2\nPreamp: -2 dB\nFilter: ON HSC 12 dB Fc 8000 Hz Gain 3 dB\n\
            Channel: all\nFilter: ON PK Fc 1000 Hz Gain 1 dB Q 1\n";
        let p = bands(parse(text).unwrap());
        assert_eq!(p[0].channels, vec![0]);
        assert!((p[0].q - std::f64::consts::SQRT_2).abs() < 1e-3);
        assert_eq!(p[1].kind, EqFilterKind::Gain);
        assert_eq!(p[1].channels, vec![1]);
        // 12 dB/oct is RBJ's S = 1, whatever the gain.
        assert!((p[2].q - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-9);
        assert!(p[3].channels.is_empty());
    }

    #[test]
    fn a_rew_filter_settings_file() {
        let text = "Filter Settings file\n\nRoom EQ V5.31.3\nDated: 4 Oct 2026 13:30:00\n\n\
            Notes:Left speaker\n\nEqualiser: Generic\nAverage 1\n\
            Filter  1: ON  PK       Fc   46.50 Hz  Gain  -9.40 dB  Q  4.470\n\
            Filter  2: ON  None\n\
            Filter  3: OFF PK       Fc   80.0 Hz  Gain  -2.00 dB  Q  2.000\n";
        let p = parse(text).unwrap();
        assert_eq!(
            p.filters,
            vec![band(EqFilterKind::Peaking, 46.5, -9.4, 4.47).into()]
        );
    }

    /// squig.link's Export, as its graph tool writes it: CRLF, shelves as
    /// LSC/HSC with a Q, and in two-channel mode a section per side with its
    /// own preamp.
    #[test]
    fn squig_link_exports() {
        let one = "Preamp: -4.1 dB\r\nFilter 1: ON LSC Fc 105 Hz Gain 3.5 dB Q 0.7\r\n\
            Filter 2: ON PK Fc 3200 Hz Gain -2.25 dB Q 1.41\r\nFilter 3: ON HSC Fc 10000 Hz Gain 1 dB Q 0.7\r\n";
        let p = parse(one).unwrap();
        assert_eq!(p.preamp_db, Some(-4.1));
        let b = bands(p);
        assert_eq!(b.len(), 3);
        assert_eq!(b[0].kind, EqFilterKind::LowShelf);
        assert_eq!(b[2].kind, EqFilterKind::HighShelf);

        let two = "Channel: L\r\nPreamp: -3 dB\r\nFilter 1: ON PK Fc 3000 Hz Gain -2 dB Q 1\r\n\r\n\
            Channel: R\r\nPreamp: -2.5 dB\r\nFilter 1: ON PK Fc 3100 Hz Gain -1.5 dB Q 1\r\n\r\n";
        let p = parse(two).unwrap();
        assert_eq!(p.preamp_db, None);
        let on = |c: u16| {
            p.filters
                .iter()
                .filter(move |f| f.channels() == [c])
                .count()
        };
        assert_eq!(
            (on(0), on(1)),
            (2, 2),
            "a gain band and a peak on each side"
        );

        // The same preamp on both sides is the profile's preamp.
        let p = parse(include_str!("testdata/aful-8s-squig-harman-2019.txt")).unwrap();
        assert_eq!(p.preamp_db, Some(-5.0));
        assert!(
            p.filters
                .iter()
                .all(|f| matches!(f, DspFilter::Band(b) if b.kind == EqFilterKind::Peaking)),
            "no gain bands left"
        );
        assert_eq!(p.filters.len(), 16, "eight peaks a side");

        // A `Copy` mixes after the preamps: they stay gain bands.
        let mixed = "Channel: L R\nPreamp: -5 dB\nCopy: L=L+C R=R+C\n";
        let p = parse(mixed).unwrap();
        assert_eq!(p.preamp_db, None);
        let gains = p
            .filters
            .iter()
            .filter(|f| matches!(f, DspFilter::Band(b) if b.kind == EqFilterKind::Gain))
            .count();
        assert_eq!(gains, 1);
    }

    /// A Qudelix 5K preset export: a type line, `//` comments, a preamp and
    /// ten filters for each channel, trailing spaces, and the headphone's
    /// impedance and sensitivity at the end.
    #[test]
    fn a_qudelix_export_is_read() {
        let qudelix = "TYPE: PEQ\r\n\r\n// SPK EQ - L\r\nChannel: L\r\nPreamp: -2.3 dB \r\n\
            Filter 1: ON LS Fc 91 Hz Gain 0.0 dB Q 0.722 \r\n\
            Filter 2: ON PK Fc 120 Hz Gain -1.2 dB Q 1.08 \r\n\
            Filter 3: ON PK Fc 380 Hz Gain 0.8 dB Q 2.1 \r\n\
            Filter 4: ON PK Fc 950 Hz Gain -0.5 dB Q 1.4 \r\n\
            Filter 5: ON PK Fc 1800 Hz Gain 1.6 dB Q 3.2 \r\n\
            Filter 6: ON PK Fc 2900 Hz Gain -2.3 dB Q 4.0 \r\n\
            Filter 7: ON PK Fc 4200 Hz Gain 1.1 dB Q 5.5 \r\n\
            Filter 8: ON PK Fc 5600 Hz Gain -1.9 dB Q 6.0 \r\n\
            Filter 9: ON PK Fc 8100 Hz Gain 0.7 dB Q 2.2 \r\n\
            Filter 10: ON HS Fc 6906 Hz Gain 0.0 dB Q 0.658 \r\n\r\n\
            // SPK EQ - R\r\nChannel: R\r\nPreamp: -1.9 dB \r\n\
            Filter 1: ON LS Fc 91 Hz Gain 0.0 dB Q 0.722 \r\n\
            Filter 2: ON PK Fc 125 Hz Gain -1.0 dB Q 1.1 \r\n\
            Filter 3: ON PK Fc 380 Hz Gain 0.8 dB Q 2.1 \r\n\
            Filter 4: ON PK Fc 950 Hz Gain -0.5 dB Q 1.4 \r\n\
            Filter 5: ON PK Fc 1800 Hz Gain 1.6 dB Q 3.2 \r\n\
            Filter 6: ON PK Fc 2900 Hz Gain -2.3 dB Q 4.0 \r\n\
            Filter 7: ON PK Fc 4200 Hz Gain 1.1 dB Q 5.5 \r\n\
            Filter 8: ON PK Fc 5600 Hz Gain -1.9 dB Q 6.0 \r\n\
            Filter 9: ON PK Fc 8100 Hz Gain 0.7 dB Q 2.2 \r\n\
            Filter 10: ON HS Fc 6906 Hz Gain 0.0 dB Q 0.658 \r\n\r\n\
            IMPEDANCE: 0.0 ohm\r\nSENSITIVITY: 0.0 dBSPL/mW\r\n";
        let p = parse(qudelix).unwrap();
        // Each side's preamp is a gain band on that side, then its ten.
        for c in [0u16, 1] {
            let side: Vec<&EqFilter> = p
                .filters
                .iter()
                .filter(|f| f.channels() == [c])
                .map(|f| match f {
                    DspFilter::Band(b) => b,
                    other => panic!("{other:?}"),
                })
                .collect();
            assert_eq!(side.len(), 11, "channel {c}");
            assert_eq!(side[0].kind, EqFilterKind::Gain);
            assert_eq!(side[0].gain_db, if c == 0 { -2.3 } else { -1.9 });
            assert_eq!(side[1].kind, EqFilterKind::LowShelf);
            assert_eq!((side[1].freq, side[1].q), (91.0, 0.722));
            assert_eq!(side[10].kind, EqFilterKind::HighShelf);
            assert_eq!((side[10].freq, side[10].q), (6906.0, 0.658));
        }
        let geq = parse("TYPE: GEQ\nChannel: L\n").unwrap_err();
        assert!(geq.contains("a GEQ preset is not supported"), "{geq}");
    }

    fn bands(p: Parsed) -> Vec<EqFilter> {
        p.filters
            .into_iter()
            .map(|f| match f {
                DspFilter::Band(b) => b,
                other => panic!("not a band: {other:?}"),
            })
            .collect()
    }

    #[test]
    fn what_koan_cannot_do_is_refused_by_name() {
        for text in [
            "Filter 1: ON IIR Order 1 Coefficients 1 0 1 0",
            "Copy: L=0.5",
            "Delay: 3 furlongs",
            "If: sampleRate == 44100\nFilter: ON PK Fc 100 Hz Gain 1 dB Q 1\nEndIf:",
            "If: sampleRate == 44100\nDelay: 1 ms\nEndIf:",
        ] {
            assert!(parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn first_order_shelves() {
        let b = bands(
            parse("Filter: ON LS 6dB Fc 100 Hz Gain 3 dB\nFilter: ON HS 6 dB Fc 9000 Hz Gain -2 dB\nFilter: ON LS 12dB Fc 100 Hz Gain 3 dB")
                .unwrap(),
        );
        assert_eq!(b[0].kind, EqFilterKind::LowShelfFirstOrder);
        assert_eq!(b[1].kind, EqFilterKind::HighShelfFirstOrder);
        assert_eq!(b[2].kind, EqFilterKind::LowShelf);
    }

    #[test]
    fn delays_on_the_selected_channels() {
        let p = parse("Channel: R\nDelay: 0.5 ms\nChannel: all\nDelay: 12 samples").unwrap();
        assert_eq!(
            p.filters,
            vec![
                DspFilter::Delay(Delay {
                    ms: 0.5,
                    channels: vec![1],
                    ..Default::default()
                }),
                DspFilter::Delay(Delay {
                    samples: 12.0,
                    ..Default::default()
                }),
            ]
        );
    }

    #[test]
    fn copy_becomes_a_mix() {
        let mix = |text: &str| match parse(text).unwrap().filters.remove(0) {
            DspFilter::Mix(m) => m.outputs,
            other => panic!("{other:?}"),
        };
        assert_eq!(mix("Copy: L=R R=L"), vec![vec![(1, 1.0)], vec![(0, 1.0)]]);
        let o = mix("Copy: R = 0.25*L + -6 dB*R");
        assert_eq!(o[0], vec![(0, 1.0)], "L is left as it was");
        assert_eq!(o[1][0], (0, 0.25));
        assert!((o[1][1].1 - 0.501187).abs() < 1e-6);
        assert_eq!(mix("Copy: L=-R")[0], vec![(1, -1.0)]);
        assert_eq!(mix("Copy: L=0")[0], vec![]);
    }

    #[test]
    fn graphic_eq_is_a_curve() {
        let p = parse("GraphicEQ: 20 -1.5; 1000 0; 20000 2.25").unwrap();
        assert_eq!(
            p.filters,
            vec![DspFilter::Graphic(GraphicEq {
                points: vec![(20.0, -1.5), (1000.0, 0.0), (20000.0, 2.25)],
                channels: vec![],
            })]
        );
        assert!(looks_like("GraphicEQ: 20 -1.5; 1000 0"));
    }

    #[test]
    fn a_copy_after_a_convolution_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        write_wav(&dir.path().join("l.wav"), 48000, &[vec![0.5]]).unwrap();
        let config = dir.path().join("config.txt");
        std::fs::write(&config, "Convolution: l.wav\nCopy: L=R\n").unwrap();
        assert!(read(&config).unwrap_err().contains("Copy"));
        std::fs::write(&config, "Copy: L=R\nConvolution: l.wav\n").unwrap();
        assert!(read(&config).is_ok());

        // Across an `Include:`, in either direction.
        let room = dir.path().join("room.txt");
        std::fs::write(&room, "Convolution: l.wav\n").unwrap();
        std::fs::write(&config, "Include: room.txt\nCopy: L=R\n").unwrap();
        assert!(read(&config).unwrap_err().contains("Copy"));
        std::fs::write(&room, "Copy: L=R\n").unwrap();
        std::fs::write(&config, "Convolution: l.wav\nInclude: room.txt\n").unwrap();
        assert!(read(&config).unwrap_err().contains("Copy"));
    }

    #[test]
    fn convolution_per_channel_through_an_include() {
        let dir = tempfile::tempdir().unwrap();
        write_wav(&dir.path().join("l.wav"), 48000, &[vec![0.5]]).unwrap();
        std::fs::write(
            dir.path().join("config.txt"),
            "Include: room.txt\nPreamp: -3 dB\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("room.txt"),
            "Channel: L\nConvolution: l.wav\n",
        )
        .unwrap();
        let p = read(&dir.path().join("config.txt")).unwrap();
        assert_eq!(p.preamp_db, Some(-3.0));
        let ir = &p.impulses[0];
        assert_eq!(ir.rate, 48000);
        // The right channel passes through.
        assert_eq!(ir.as_channels().unwrap(), vec![&[0.5][..], &[1.0][..]]);
    }
}
