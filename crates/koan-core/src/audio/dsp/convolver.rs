//! Convolver's `.cfg`, which Roon, Acourate, Audiolense and Home Audio Fidelity
//! filter packs use to say which response goes with which channel. See
//! <https://convolver.sourceforge.net/config.html>.
//!
//! ```text
//! 44100 2 2 0          rate, inputs, outputs, output channel mask (hex)
//! 0 0                  input delays, ms
//! 0 0                  output delays, ms
//! left.wav             then four lines per route: file,
//! 0                      channel within the file,
//! 0.0                    inputs mixed in,
//! 0.0                    outputs added to
//! ```
//!
//! A mapping token is a channel and a weight in one number: `1.0` is channel 1
//! at full weight, `0.5` channel 0 at half, `-0.99999` channel 0 inverted. The
//! delays are read as a run of numbers whatever lines they fall on: the spec
//! puts them on one line and every real file on two. A `.cfg` whose first line
//! is not a header is a list of other files, one per line.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;

use super::impulse::{Impulse, Route, read_audio};
use super::raw::{self, RawFormat};

/// Every response a `.cfg` describes: one, or several for a list.
pub fn read(path: &Path) -> Result<Vec<Impulse>, String> {
    read_depth(path, 0)
}

fn read_depth(path: &Path, depth: usize) -> Result<Vec<Impulse>, String> {
    if depth > 4 {
        return Err("lists nested too deeply".into());
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let dir = path.parent().unwrap_or(Path::new("."));
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();
    let Some(first) = lines.first() else {
        return Err(format!("{}: empty", path.display()));
    };
    if header(first).is_none() {
        let mut out = Vec::new();
        for name in &lines {
            let file = dir.join(name.replace('\\', "/"));
            if is_cfg(&file) {
                out.extend(read_depth(&file, depth + 1)?);
            } else {
                let (rate, channels) =
                    read_audio(&file).map_err(|e| format!("{}: {e}", file.display()))?;
                out.push(Impulse::from_channels(rate, channels));
            }
        }
        return Ok(out);
    }
    parse(&lines, dir)
        .map(|i| vec![i])
        .map_err(|e| format!("{}: {e}", path.display()))
}

pub fn is_cfg(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("cfg"))
}

fn header(line: &str) -> Option<(u32, usize, usize)> {
    let t: Vec<&str> = line.split_whitespace().collect();
    if t.len() != 4 {
        return None;
    }
    u32::from_str_radix(t[3], 16).ok()?;
    Some((t[0].parse().ok()?, t[1].parse().ok()?, t[2].parse().ok()?))
}

fn parse(lines: &[&str], dir: &Path) -> Result<Impulse, String> {
    let (rate, inputs, outputs) = header(lines[0]).ok_or("bad header")?;
    if inputs != outputs {
        return Err(format!(
            "maps {inputs} channels to {outputs}; koan plays as many channels as it decodes, so only filters with as many outputs as inputs"
        ));
    }
    if rate == 0 || inputs == 0 {
        return Err("bad header".into());
    }

    let mut delays = Vec::new();
    let mut next = 1;
    while delays.len() < inputs + outputs {
        let line = lines.get(next).ok_or("ends before its delays")?;
        for t in line.split_whitespace() {
            delays.push(t.parse::<f64>().map_err(|_| format!("bad delay: {t}"))?);
        }
        next += 1;
    }
    if delays.len() != inputs + outputs {
        return Err("delays do not match the channel counts".into());
    }
    let frames = |ms: f64| (ms * rate as f64 / 1000.0).round().max(0.0) as usize;
    let in_delays = delays[..inputs].iter().map(|&d| frames(d)).collect();
    let out_delays = delays[inputs..].iter().map(|&d| frames(d)).collect();

    let blocks = &lines[next..];
    if blocks.is_empty() || !blocks.len().is_multiple_of(4) {
        return Err(
            "routes are four lines each: file, channel in the file, inputs, outputs".into(),
        );
    }
    let mut files: HashMap<String, Vec<Vec<f32>>> = HashMap::new();
    let mut routes = Vec::new();
    for block in blocks.chunks(4) {
        let name = block[0].replace('\\', "/");
        if !files.contains_key(&name) {
            files.insert(name.clone(), load(&dir.join(&name), rate)?);
        }
        let channel: usize = block[1]
            .parse()
            .map_err(|_| format!("bad channel: {}", block[1]))?;
        let ir = files[&name]
            .get(channel)
            .ok_or_else(|| format!("{name} has no channel {channel}"))?
            .clone();
        routes.push(Route {
            ir,
            inputs: mapping(block[2], inputs)?,
            outputs: mapping(block[3], outputs)?,
        });
    }
    Ok(Impulse {
        rate,
        channels: Some(inputs),
        routes,
        in_delays,
        out_delays,
    })
}

/// A response file named by a `.cfg`. Raw `.pcm` is 32-bit float and `.dbl`
/// 64-bit, both mono and at the header's rate, as Roon reads them.
fn load(path: &Path, rate: u32) -> Result<Vec<Vec<f32>>, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    let raw_format = match ext.as_deref() {
        Some("pcm") => Some(RawFormat::F32Le),
        Some("dbl") => Some(RawFormat::F64Le),
        _ => None,
    };
    if let Some(format) = raw_format {
        return Ok(vec![
            raw::read(path, format, 0, 0).map_err(|e| format!("{}: {e}", path.display()))?,
        ]);
    }
    let (file_rate, channels) = read_audio(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if file_rate != rate {
        return Err(format!(
            "{} is at {file_rate} Hz but the cfg is for {rate} Hz",
            path.display()
        ));
    }
    Ok(channels)
}

fn mapping(line: &str, channels: usize) -> Result<Vec<(usize, f32)>, String> {
    line.split_whitespace()
        .map(|t| {
            let (ch, w) = token(t).ok_or_else(|| format!("bad channel mapping: {t}"))?;
            if ch >= channels {
                return Err(format!("channel {ch} in \"{line}\", which has {channels}"));
            }
            Ok((ch, w))
        })
        .collect()
}

/// `2.5` → channel 2 at 0.5; `1.0` → channel 1 at 1; `-0.99999` → channel 0
/// inverted. A fraction of zero means full weight.
fn token(t: &str) -> Option<(usize, f32)> {
    let (negative, t) = match t.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, t),
    };
    let (ch, frac) = t.split_once('.').unwrap_or((t, ""));
    let ch = ch.parse().ok()?;
    let weight = if frac.trim_end_matches('0').is_empty() {
        1.0
    } else {
        format!("0.{frac}").parse::<f32>().ok()?
    };
    let weight = if negative {
        // `-0.99999` is the spec's spelling of inverted at full weight.
        if weight > 0.9999 { -1.0 } else { -weight }
    } else {
        weight
    };
    Some((ch, weight))
}

fn format_token(ch: usize, w: f32) -> String {
    let magnitude = w.abs();
    let frac = if (magnitude - 1.0).abs() < 1e-6 {
        if w < 0.0 {
            "99999".to_string()
        } else {
            "0".to_string()
        }
    } else {
        let s = format!("{magnitude:.6}");
        s.split_once('.')
            .map_or("0", |(_, f)| f)
            .trim_end_matches('0')
            .to_string()
    };
    format!("{}{ch}.{frac}", if w < 0.0 { "-" } else { "" })
}

/// Write `impulse` as `<stem>.cfg` beside `<stem>.wav`, which holds one route's
/// response per channel. Weights above 1 cannot be written in this format.
pub fn write(dir: &Path, stem: &str, impulse: &Impulse) -> Result<std::path::PathBuf, String> {
    let channels = impulse.channels.unwrap_or(1);
    let wav = format!("{stem}.wav");
    let irs: Vec<Vec<f32>> = impulse.routes.iter().map(|r| r.ir.clone()).collect();
    raw::write_wav(&dir.join(&wav), impulse.rate, &irs).map_err(|e| e.to_string())?;

    let ms = |d: &[usize]| {
        (0..channels)
            .map(|c| {
                let f = d.get(c).copied().unwrap_or(0);
                format!("{}", f as f64 * 1000.0 / impulse.rate as f64)
            })
            .collect::<Vec<_>>()
            .join(" ")
    };
    let mut cfg = format!("{} {channels} {channels} 0\n", impulse.rate);
    let _ = writeln!(cfg, "{}", ms(&impulse.in_delays));
    let _ = writeln!(cfg, "{}", ms(&impulse.out_delays));
    for (k, r) in impulse.routes.iter().enumerate() {
        let tokens = |m: &[(usize, f32)]| -> Result<String, String> {
            m.iter()
                .map(|&(c, w)| {
                    if w.abs() > 1.0 + 1e-6 {
                        Err(format!("a weight of {w} cannot be written to a .cfg"))
                    } else {
                        Ok(format_token(c, w))
                    }
                })
                .collect::<Result<Vec<_>, _>>()
                .map(|t| t.join(" "))
        };
        let _ = writeln!(
            cfg,
            "{wav}\n{k}\n{}\n{}",
            tokens(&r.inputs)?,
            tokens(&r.outputs)?
        );
    }
    let path = dir.join(format!("{stem}.cfg"));
    std::fs::write(&path, cfg).map_err(|e| e.to_string())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapping_tokens() {
        assert_eq!(token("0.0"), Some((0, 1.0)));
        assert_eq!(token("1.0"), Some((1, 1.0)));
        assert_eq!(token("0.5"), Some((0, 0.5)));
        assert_eq!(token("2.25"), Some((2, 0.25)));
        assert_eq!(token("-0.99999"), Some((0, -1.0)));
        assert_eq!(token("-1.5"), Some((1, -0.5)));
        assert_eq!(token("3"), Some((3, 1.0)));
        for (c, w) in [(0, 1.0), (1, 0.5), (2, -1.0), (1, -0.25)] {
            assert_eq!(token(&format_token(c, w)), Some((c, w)));
        }
    }

    fn wav(dir: &Path, name: &str, rate: u32, channels: &[Vec<f32>]) {
        raw::write_wav(&dir.join(name), rate, channels).unwrap();
    }

    #[test]
    fn a_roon_style_cfg_with_one_file_per_side() {
        let dir = tempfile::tempdir().unwrap();
        wav(dir.path(), "L44.wav", 44100, &[vec![1.0, 0.5]]);
        wav(dir.path(), "R44.wav", 44100, &[vec![0.25, 0.125]]);
        std::fs::write(
            dir.path().join("44.cfg"),
            "44100 2 2 0\n0 0 \n0 0 \n\nL44.wav\n0\n0.0\n0.0\n\nR44.wav\n0\n1.0\n1.0\n",
        )
        .unwrap();
        let ir = read(&dir.path().join("44.cfg")).unwrap().remove(0);
        assert_eq!(ir.rate, 44100);
        assert_eq!(ir.channels, Some(2));
        assert_eq!(
            ir.as_channels().unwrap(),
            vec![&[1.0, 0.5][..], &[0.25, 0.125][..]]
        );
    }

    #[test]
    fn crossfeed_and_delays_survive_a_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        wav(dir.path(), "hl.wav", 48000, &[vec![1.0], vec![0.5]]);
        std::fs::write(
            dir.path().join("x.cfg"),
            "48000 2 2 0\n0 0 0 10\nhl.wav\n0\n0.0\n0.0\nhl.wav\n1\n0.0\n1.5\n",
        )
        .unwrap();
        let ir = read(&dir.path().join("x.cfg")).unwrap().remove(0);
        assert_eq!(ir.routes[1].outputs, vec![(1, 0.5)]);
        assert_eq!(ir.out_delays, vec![0, 480]);
        assert!(ir.as_channels().is_none());

        let out = tempfile::tempdir().unwrap();
        let written = write(out.path(), "48000", &ir).unwrap();
        assert_eq!(read(&written).unwrap().remove(0), ir);
    }

    #[test]
    fn more_outputs_than_inputs_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("x.cfg"), "44100 2 4 33\n0 0\n20 30 0 0\n").unwrap();
        let err = read(&dir.path().join("x.cfg")).unwrap_err();
        assert!(err.contains("2 channels to 4"), "{err}");
    }

    #[test]
    fn a_list_names_the_cfgs_for_each_rate() {
        let dir = tempfile::tempdir().unwrap();
        wav(dir.path(), "a.wav", 44100, &[vec![1.0]]);
        wav(dir.path(), "b.wav", 96000, &[vec![1.0], vec![1.0]]);
        std::fs::write(dir.path().join("list.cfg"), "a.wav\nb.wav\n").unwrap();
        let irs = read(&dir.path().join("list.cfg")).unwrap();
        assert_eq!(
            irs.iter().map(|i| i.rate).collect::<Vec<_>>(),
            vec![44100, 96000]
        );
    }
}
