//! Equalizer APO's `config.txt`, which is also what AutoEQ's
//! `ParametricEQ.txt` and REW's filter exports are written in. See
//! <https://sourceforge.net/p/equalizerapo/wiki/Configuration%20reference/>.
//!
//! ```text
//! Preamp: -6.8 dB
//! Filter 1: ON PK Fc 20 Hz Gain -1.3 dB Q 2.000
//! Channel: R
//! Filter: ON LSC 6 dB Fc 105 Hz Gain 5.5 dB
//! Convolution: room-48k.wav
//! ```
//!
//! What has no equivalent in koan is refused by name rather than skipped:
//! dropping a band, or a delay, changes the correction without saying so.

use std::collections::BTreeMap;
use std::path::Path;

use super::impulse::{Impulse, Route, read_audio};
use crate::config::{EqFilter, EqFilterKind};

#[derive(Debug, Default, PartialEq)]
pub struct Parsed {
    /// What `Preamp:` asked for across every channel.
    pub preamp_db: Option<f64>,
    pub filters: Vec<EqFilter>,
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
    Ok(parsed)
}

/// Text without a file behind it — pasted, or shared from a chat. A
/// `Convolution:` or `Include:` in it has nothing to be relative to.
pub fn parse(text: &str) -> Result<Parsed, String> {
    let mut parsed = Parsed::default();
    let mut convolutions = Convolutions::default();
    parse_into(text, None, &mut parsed, &mut convolutions, 0)?;
    parsed.impulses = convolutions.build();
    Ok(parsed)
}

/// Whether `text` reads as Equalizer APO configuration.
pub fn looks_like(text: &str) -> bool {
    text.lines().map(str::trim).any(|l| {
        (l.starts_with("Filter") && l.contains(':'))
            || l.starts_with("Preamp:")
            || l.starts_with("Convolution:")
            || l.starts_with("Include:")
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
        if line.is_empty() || line.starts_with('#') {
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
        match command.split_whitespace().next().unwrap_or("") {
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
                    Some(channels) => parsed.filters.push(EqFilter {
                        kind: EqFilterKind::Gain,
                        freq: 1000.0,
                        gain_db: db,
                        q: 1.0,
                        channels: channels.clone(),
                    }),
                }
            }
            "Filter" => {
                if conditional > 0 {
                    return Err(fail(
                        "bands that depend on the sample rate are not supported",
                    ));
                }
                if let Some(mut filter) = filter(rest).map_err(|e| fail(&e))? {
                    filter.channels = selection.clone().unwrap_or_default();
                    parsed.filters.push(filter);
                }
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
            "GraphicEQ" => {
                return Err(fail(
                    "GraphicEQ is a curve, not filters; import the parametric version instead (AutoEQ's ParametricEQ.txt, or squig.link's Export rather than Export Graphic EQ)",
                ));
            }
            other => return Err(fail(&format!("{other} is not supported"))),
        }
    }
    Ok(())
}

impl Convolutions {
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
    // 6 dB ones are first-order, which koan does not build.
    if matches!(kind_word, "LS" | "HS") {
        match words.get(2) {
            Some(&"6dB") => return Err("first-order (6dB) shelves are not supported".into()),
            Some(&"12dB") => {}
            _ => {}
        }
    }
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
            vec![
                band(EqFilterKind::Peaking, 20.0, -1.3, 2.0),
                band(EqFilterKind::LowShelf, 105.0, 5.5, 0.7),
                band(
                    EqFilterKind::HighShelf,
                    10000.0,
                    2.5,
                    std::f64::consts::FRAC_1_SQRT_2
                ),
            ]
        );
    }

    #[test]
    fn channel_sections_and_slopes() {
        let text = "Channel: L\nFilter: ON PK Fc 100 Hz Gain -3 dB BW Oct 1.0\n\
            Channel: 2\nPreamp: -2 dB\nFilter: ON HSC 12 dB Fc 8000 Hz Gain 3 dB\n\
            Channel: all\nFilter: ON PK Fc 1000 Hz Gain 1 dB Q 1\n";
        let p = parse(text).unwrap();
        assert_eq!(p.preamp_db, None);
        assert_eq!(p.filters[0].channels, vec![0]);
        assert!((p.filters[0].q - std::f64::consts::SQRT_2).abs() < 1e-3);
        assert_eq!(p.filters[1].kind, EqFilterKind::Gain);
        assert_eq!(p.filters[1].channels, vec![1]);
        // 12 dB/oct is RBJ's S = 1, whatever the gain.
        assert!((p.filters[2].q - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-9);
        assert!(p.filters[3].channels.is_empty());
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
            vec![band(EqFilterKind::Peaking, 46.5, -9.4, 4.47)]
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
        assert_eq!(p.filters.len(), 3);
        assert_eq!(p.filters[0].kind, EqFilterKind::LowShelf);
        assert_eq!(p.filters[2].kind, EqFilterKind::HighShelf);

        let two = "Channel: L\r\nPreamp: -3 dB\r\nFilter 1: ON PK Fc 3000 Hz Gain -2 dB Q 1\r\n\r\n\
            Channel: R\r\nPreamp: -2.5 dB\r\nFilter 1: ON PK Fc 3100 Hz Gain -1.5 dB Q 1\r\n\r\n";
        let p = parse(two).unwrap();
        assert_eq!(p.preamp_db, None);
        let on = |c: u16| {
            p.filters
                .iter()
                .filter(move |f| f.channels == vec![c])
                .count()
        };
        assert_eq!(
            (on(0), on(1)),
            (2, 2),
            "a gain band and a peak on each side"
        );
    }

    #[test]
    fn what_koan_cannot_do_is_refused_by_name() {
        for text in [
            "Filter 1: ON IIR Order 1 Coefficients 1 0 1 0",
            "Delay: 10 ms",
            "Copy: L=R",
            "GraphicEQ: 20 -1; 30 0",
            "If: sampleRate == 44100\nFilter: ON PK Fc 100 Hz Gain 1 dB Q 1\nEndIf:",
        ] {
            assert!(parse(text).is_err(), "{text}");
        }
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
