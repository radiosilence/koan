//! CamillaDSP configuration: its biquads become bands and its `Conv` filters
//! responses, at the rate the config runs at (`devices.samplerate`). See
//! <https://github.com/HEnquist/camilladsp>.
//!
//! Pipeline steps name the channels a filter applies to as `channel: 0` up to
//! v2 and `channels: [0, 1]` from v3; both are read. Mixers and delays keep
//! their place in the pipeline. What koan does not build is refused by name: a
//! config that half-imports corrects something other than what it was
//! measured for. That includes a mixer that changes the channel count, since
//! koan plays as many channels as the source has, and a mixer after a `Conv`,
//! since responses run last.

use std::path::Path;

use yaml_rust2::{Yaml, YamlLoader};

use super::apo::{Convolutions, Parsed, q_from_bandwidth, q_from_slope};
use super::impulse::read_audio;
use super::raw::{self, RawFormat};
use crate::config::{Delay, DspFilter, EqFilter, EqFilterKind, Mix};

pub fn read(path: &Path) -> Result<Parsed, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(&text, path.parent()).map_err(|e| format!("{}: {e}", path.display()))
}

/// Whether `text` reads as a CamillaDSP config.
pub fn looks_like(text: &str) -> bool {
    YamlLoader::load_from_str(text)
        .ok()
        .and_then(|docs| docs.into_iter().next())
        .is_some_and(|doc| !doc["pipeline"].is_badvalue() || !doc["filters"].is_badvalue())
}

fn number(y: &Yaml) -> Option<f64> {
    match y {
        Yaml::Integer(i) => Some(*i as f64),
        Yaml::Real(_) => y.as_f64(),
        _ => None,
    }
}

pub fn parse(text: &str, dir: Option<&Path>) -> Result<Parsed, String> {
    let doc = YamlLoader::load_from_str(text)
        .map_err(|e| e.to_string())?
        .into_iter()
        .next()
        .ok_or("empty")?;
    let rate = doc["devices"]["samplerate"]
        .as_i64()
        .ok_or("no devices.samplerate")? as u32;
    let channels = doc["devices"]["capture"]["channels"].as_i64().unwrap_or(2) as u16;
    let filters = &doc["filters"];

    let mut parsed = Parsed::default();
    let mut convolutions = Convolutions::default();
    let mut convolved = false;
    let steps = doc["pipeline"].as_vec().cloned().unwrap_or_default();
    for (n, step) in steps.iter().enumerate() {
        let fail = |why: String| format!("pipeline step {}: {why}", n + 1);
        if step["bypassed"].as_bool() == Some(true) {
            continue;
        }
        match step["type"].as_str() {
            Some("Filter") => {}
            Some("Mixer") => {
                if convolved {
                    return Err(fail(
                        "a mixer after a Conv filter is not supported: responses run last".into(),
                    ));
                }
                let name = step["name"]
                    .as_str()
                    .ok_or_else(|| fail("no mixer name".into()))?;
                let m =
                    mixer(&doc["mixers"][name]).map_err(|e| fail(format!("mixer {name}: {e}")))?;
                parsed.filters.push(DspFilter::Mix(m));
                continue;
            }
            Some(other) => return Err(fail(format!("{other} steps are not supported"))),
            None => return Err(fail("no type".into())),
        }
        let selected: Vec<u16> = match (&step["channels"], &step["channel"]) {
            (Yaml::Array(list), _) => list
                .iter()
                .filter_map(|c| c.as_i64())
                .map(|c| c as u16)
                .collect(),
            (_, Yaml::Integer(c)) => vec![*c as u16],
            _ => (0..channels).collect(),
        };
        // Every channel is the same as none named, to the bands.
        let band_channels = if selected.len() == channels as usize {
            Vec::new()
        } else {
            selected.clone()
        };
        for name in step["names"].as_vec().into_iter().flatten() {
            let name = name
                .as_str()
                .ok_or_else(|| fail("a filter name is not text".into()))?;
            let filter = &filters[name];
            if filter.is_badvalue() {
                return Err(fail(format!("no filter called {name}")));
            }
            let fail = |why: String| format!("filter {name}: {why}");
            let p = &filter["parameters"];
            match filter["type"].as_str() {
                Some("Biquad") => {
                    let mut band = biquad(p).map_err(fail)?;
                    band.channels = band_channels.clone();
                    parsed.filters.push(band.into());
                }
                Some("Gain") => {
                    if p["inverted"].as_bool() == Some(true) || p["mute"].as_bool() == Some(true) {
                        return Err(fail("inverted or muted gain is not supported".into()));
                    }
                    let gain = number(&p["gain"]).ok_or_else(|| fail("no gain".into()))?;
                    let gain_db = if p["scale"].as_str() == Some("linear") {
                        20.0 * gain.abs().log10()
                    } else {
                        gain
                    };
                    parsed.filters.push(DspFilter::Band(EqFilter {
                        kind: EqFilterKind::Gain,
                        freq: 1000.0,
                        gain_db,
                        q: 1.0,
                        channels: band_channels.clone(),
                    }));
                }
                Some("Delay") => {
                    let mut d = delay(p, rate).map_err(fail)?;
                    d.channels = band_channels.clone();
                    parsed.filters.push(DspFilter::Delay(d));
                }
                Some("Conv") => {
                    let ir = conv(p, dir, rate, channels).map_err(fail)?;
                    if let Some(ir) = ir {
                        convolutions.add(rate, vec![ir], Some(&selected));
                        convolved = true;
                    }
                }
                Some(other) => return Err(fail(format!("{other} filters are not supported"))),
                None => return Err(fail("no type".into())),
            }
        }
    }
    parsed.impulses = convolutions.build();
    Ok(parsed)
}

fn biquad(p: &Yaml) -> Result<EqFilter, String> {
    let kind_name = p["type"].as_str().ok_or("no biquad type")?;
    let kind = match kind_name {
        "Peaking" => EqFilterKind::Peaking,
        "Lowshelf" => EqFilterKind::LowShelf,
        "Highshelf" => EqFilterKind::HighShelf,
        "Lowpass" => EqFilterKind::LowPass,
        "Highpass" => EqFilterKind::HighPass,
        "Notch" => EqFilterKind::Notch,
        "Bandpass" => EqFilterKind::BandPass,
        "Allpass" => EqFilterKind::AllPass,
        "LowshelfFO" => EqFilterKind::LowShelfFirstOrder,
        "HighshelfFO" => EqFilterKind::HighShelfFirstOrder,
        "LowpassFO" => EqFilterKind::LowPassFirstOrder,
        "HighpassFO" => EqFilterKind::HighPassFirstOrder,
        "AllpassFO" => EqFilterKind::AllPassFirstOrder,
        other => return Err(format!("{other} biquads are not supported")),
    };
    let freq = number(&p["freq"]).ok_or("no freq")?;
    let gain_db = number(&p["gain"]).unwrap_or(0.0);
    let q = match (
        number(&p["q"]),
        number(&p["bandwidth"]),
        number(&p["slope"]),
    ) {
        (Some(q), _, _) => q,
        (None, Some(bw), _) => q_from_bandwidth(bw),
        (None, None, Some(slope)) => q_from_slope(gain_db, slope),
        _ => std::f64::consts::FRAC_1_SQRT_2,
    };
    Ok(EqFilter {
        kind,
        freq,
        gain_db,
        q,
        channels: Vec::new(),
    })
}

/// `delay` in `unit`: ms (the default), mm of sound at 343 m/s, or samples at
/// the config's rate.
fn delay(p: &Yaml, rate: u32) -> Result<Delay, String> {
    let value = number(&p["delay"]).ok_or("no delay")?;
    let ms = match p["unit"].as_str().unwrap_or("ms") {
        "ms" => value,
        "mm" => value / 343.0,
        "samples" => value * 1000.0 / rate as f64,
        other => return Err(format!("{other} is not a delay unit")),
    };
    Ok(Delay {
        ms,
        subsample: p["subsample"].as_bool() == Some(true),
        ..Default::default()
    })
}

/// A mixer's mapping. A destination no mapping names is silent, as it is in
/// CamillaDSP.
fn mixer(m: &Yaml) -> Result<Mix, String> {
    if m.is_badvalue() {
        return Err("not defined".into());
    }
    let count = |k: &str| m["channels"][k].as_i64().ok_or(format!("no channels.{k}"));
    let (inputs, outputs) = (count("in")?, count("out")?);
    if inputs != outputs {
        return Err(format!(
            "{inputs} channels in and {outputs} out; koan plays as many channels as the source has"
        ));
    }
    let mut rows = vec![Vec::new(); outputs as usize];
    for map in m["mapping"].as_vec().into_iter().flatten() {
        let dest = map["dest"].as_i64().ok_or("a mapping has no dest")?;
        let row = rows
            .get_mut(dest as usize)
            .ok_or(format!("dest {dest} is past the mixer's outputs"))?;
        if map["mute"].as_bool() == Some(true) {
            continue;
        }
        for src in map["sources"].as_vec().into_iter().flatten() {
            if src["mute"].as_bool() == Some(true) {
                continue;
            }
            let channel = src["channel"].as_i64().ok_or("a source has no channel")?;
            let gain = number(&src["gain"]).unwrap_or(0.0);
            let mut gain = match src["scale"].as_str().unwrap_or("dB") {
                "linear" => gain,
                _ => 10f64.powf(gain / 20.0),
            };
            if src["inverted"].as_bool() == Some(true) {
                gain = -gain;
            }
            row.push((channel as u16, gain));
        }
    }
    Ok(Mix { outputs: rows })
}

/// A `Conv` filter's response. `None` for `Dummy`, which passes audio as it is.
fn conv(
    p: &Yaml,
    dir: Option<&Path>,
    rate: u32,
    channels: u16,
) -> Result<Option<Vec<f32>>, String> {
    let file = || -> Result<std::path::PathBuf, String> {
        let name = p["filename"]
            .as_str()
            .ok_or("no filename")?
            .replace("$samplerate$", &rate.to_string())
            .replace("$channels$", &channels.to_string());
        let path = Path::new(&name);
        if path.is_absolute() {
            return Ok(path.to_path_buf());
        }
        Ok(dir
            .ok_or("names a file, and this text has no folder to find it in")?
            .join(path))
    };
    let count = |key: &str| p[key].as_i64().unwrap_or(0).max(0) as usize;
    match p["type"].as_str() {
        Some("Wav") => {
            let path = file()?;
            let (_, chans) = read_audio(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let channel = count("channel");
            chans
                .into_iter()
                .nth(channel)
                .map(Some)
                .ok_or_else(|| format!("{} has no channel {channel}", path.display()))
        }
        Some("Raw") => {
            let format_name = p["format"].as_str().unwrap_or("TEXT");
            let format = raw_format(format_name)
                .ok_or_else(|| format!("{format_name} is not a sample format koan reads"))?;
            let path = file()?;
            raw::read(
                &path,
                format,
                count("skip_bytes_lines"),
                count("read_bytes_lines"),
            )
            .map(Some)
            .map_err(|e| format!("{}: {e}", path.display()))
        }
        Some("Values") => {
            let values: Vec<f32> = p["values"]
                .as_vec()
                .ok_or("no values")?
                .iter()
                .filter_map(number)
                .map(|v| v as f32)
                .collect();
            Ok(Some(values))
        }
        Some("Dummy") => Ok(None),
        Some(other) => Err(format!("{other} convolution is not supported")),
        None => Err("no conv type".into()),
    }
}

/// Sample format names before v4 and after.
fn raw_format(name: &str) -> Option<RawFormat> {
    RawFormat::from_name(name).or(match name {
        "S16_LE" => Some(RawFormat::S16Le),
        "S24_3_LE" => Some(RawFormat::S24Le3),
        "S24_4_RJ_LE" => Some(RawFormat::S24Le),
        "S32_LE" => Some(RawFormat::S32Le),
        "F32_LE" => Some(RawFormat::F32Le),
        "F64_LE" => Some(RawFormat::F64Le),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const V3: &str = r#"
devices:
  samplerate: 96000
  capture: { type: Stdin, channels: 2, format: S32LE }
filters:
  bass:
    type: Biquad
    parameters: { type: Lowshelf, freq: 105, gain: 6, slope: 12 }
  dip:
    type: Biquad
    parameters: { type: Peaking, freq: 2000, gain: -3.5, bandwidth: 1 }
  trim:
    type: Gain
    parameters: { gain: -2 }
  room_l:
    type: Conv
    parameters: { type: Raw, filename: "room_l_$samplerate$.txt", format: TEXT }
  room_r:
    type: Conv
    parameters: { type: Values, values: [0.5, 0.25] }
pipeline:
  - type: Filter
    channels: [0, 1]
    names: [bass]
  - type: Filter
    channels: [1]
    names: [dip, trim]
  - type: Filter
    channels: [0]
    names: [room_l]
  - type: Filter
    channels: [1]
    names: [room_r]
"#;

    #[test]
    fn a_v3_config_imports() {
        let dir = tempfile::tempdir().unwrap();
        let coeffs: String = (0..20)
            .map(|i| {
                if i == 0 {
                    "1.0\n".into()
                } else {
                    "0\n".to_string()
                }
            })
            .collect();
        std::fs::write(dir.path().join("room_l_96000.txt"), coeffs).unwrap();
        let p = parse(V3, Some(dir.path())).unwrap();
        let b = bands(&p);
        assert_eq!(b.len(), 3);
        assert!(b[0].channels.is_empty(), "both channels is every channel");
        assert!((b[0].q - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-9);
        assert_eq!(b[1].channels, vec![1]);
        assert_eq!(b[2].kind, EqFilterKind::Gain);
        let ir = &p.impulses[0];
        assert_eq!(ir.rate, 96000);
        let chans = ir.as_channels().unwrap();
        assert_eq!(chans[0].len(), 20);
        assert_eq!(chans[1], &[0.5, 0.25][..]);
    }

    #[test]
    fn a_v1_step_names_one_channel() {
        let text = "devices: { samplerate: 44100 }\nfilters:\n  p: { type: Biquad, parameters: { type: Peaking, freq: 100, gain: 1, q: 1 } }\npipeline:\n  - type: Filter\n    channel: 1\n    names: [p]\n";
        let p = parse(text, None).unwrap();
        assert_eq!(p.filters[0].channels(), [1]);
    }

    fn bands(p: &Parsed) -> Vec<&EqFilter> {
        p.filters
            .iter()
            .filter_map(|f| match f {
                DspFilter::Band(b) => Some(b),
                _ => None,
            })
            .collect()
    }

    const MIXED: &str = r#"
devices: { samplerate: 48000, capture: { channels: 2 } }
mixers:
  blend:
    channels: { in: 2, out: 2 }
    mapping:
      - dest: 0
        sources:
          - { channel: 0, gain: 0 }
          - { channel: 1, gain: -6, inverted: true }
      - dest: 1
        sources:
          - { channel: 1, gain: 0.5, scale: linear }
  up:
    channels: { in: 2, out: 4 }
    mapping: []
filters:
  d: { type: Delay, parameters: { delay: 96, unit: samples, subsample: true } }
  s: { type: Biquad, parameters: { type: LowshelfFO, freq: 300, gain: 2 } }
  c: { type: Conv, parameters: { type: Values, values: [1.0] } }
pipeline:
  - type: Filter
    channels: [1]
    names: [d]
  - type: Mixer
    name: blend
  - type: Filter
    names: [s, c]
"#;

    #[test]
    fn mixers_and_delays_keep_their_place() {
        let p = parse(MIXED, None).unwrap();
        let DspFilter::Delay(d) = &p.filters[0] else {
            panic!("{:?}", p.filters[0])
        };
        assert_eq!(
            (d.ms, d.subsample, d.channels.clone()),
            (2.0, true, vec![1])
        );
        let DspFilter::Mix(m) = &p.filters[1] else {
            panic!("{:?}", p.filters[1])
        };
        assert_eq!(m.outputs[0][0], (0, 1.0));
        assert!((m.outputs[0][1].1 + 0.501187).abs() < 1e-6);
        assert_eq!(m.outputs[1], vec![(1, 0.5)]);
        assert_eq!(bands(&p)[0].kind, EqFilterKind::LowShelfFirstOrder);
        assert_eq!(p.impulses.len(), 1);
    }

    #[test]
    fn what_koan_cannot_do_is_refused() {
        let more = MIXED.replace("name: blend", "name: up");
        assert!(parse(&more, None).unwrap_err().contains("4 out"));
        let late = MIXED.replace(
            "  - type: Mixer\n    name: blend\n  - type: Filter\n    names: [s, c]\n",
            "  - type: Filter\n    names: [s, c]\n  - type: Mixer\n    name: blend\n",
        );
        assert!(parse(&late, None).unwrap_err().contains("after a Conv"));
        let free = "devices: { samplerate: 44100 }\nfilters:\n  f: { type: Biquad, parameters: { type: Free, a1: 0, a2: 0, b0: 1, b1: 0, b2: 0 } }\npipeline:\n  - type: Filter\n    names: [f]\n";
        assert!(parse(free, None).unwrap_err().contains("Free"));
    }
}
