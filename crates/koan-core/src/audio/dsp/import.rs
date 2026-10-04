//! Correction filters as other tools write them, made into a profile.
//!
//! Whatever is handed over — files, a folder, a zip as Roon takes one, or text
//! pasted from a chat — is sorted by what it is. A configuration that names
//! its files (a Convolver `.cfg`, a CamillaDSP YAML, an Equalizer APO
//! `Convolution:`) decides which file goes with which rate and channel, and
//! loose responses beside it are taken to be the ones it names. Without one,
//! each response's own rate is used, and mono files at one rate are matched to
//! channels by `L`/`R` in their names.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use thiserror::Error;

use super::impulse::{Impulse, read_audio};
use super::raw::{self, RawFormat};
use super::{apo, camilla, convolver};
use crate::config::DspFilter;

#[derive(Debug, Error)]
pub enum ImportError {
    /// Headerless coefficients, and nothing saying what rate they are at.
    #[error("{0} does not say what sample rate it is at")]
    NeedsRate(String),
    #[error("{0}")]
    Failed(String),
}

impl From<String> for ImportError {
    fn from(s: String) -> Self {
        Self::Failed(s)
    }
}

#[derive(Debug, Default)]
pub struct Imported {
    /// A name for the profile, from the file it came from.
    pub name: String,
    /// What it was imported from, by file name.
    pub source: Vec<String>,
    pub filters: Vec<DspFilter>,
    pub impulses: Vec<Impulse>,
}

const AUDIO: &[&str] = &["wav", "wave", "flac", "aif", "aiff", "aifc", "m4a", "caf"];

/// Import files, folders and zips as one profile. `rate` is for headerless
/// coefficients that say nothing of it themselves.
pub fn import(paths: &[PathBuf], rate: Option<u32>) -> Result<Imported, ImportError> {
    let scratch = Scratch::new()?;
    let mut files = Vec::new();
    for path in paths {
        gather(path, &scratch, &mut files, true)?;
    }
    let name = paths
        .first()
        .map(|p| profile_name(p))
        .unwrap_or_else(|| "Imported".into());
    let mut imported = sort(files, rate)?;
    imported.name = name;
    imported.source = paths.iter().map(|p| file_name(p)).collect();
    Ok(imported)
}

/// Import text with no file behind it: Equalizer APO or AutoEQ lines,
/// CamillaDSP YAML, or a list of coefficients.
pub fn import_text(text: &str, rate: Option<u32>) -> Result<Imported, ImportError> {
    let parsed = if apo::looks_like(text) {
        apo::parse(text)?
    } else if camilla::looks_like(text) {
        camilla::parse(text, None)?
    } else if raw::looks_like_coefficients(text) {
        let rate = rate.ok_or_else(|| ImportError::NeedsRate("The coefficients".into()))?;
        apo::Parsed {
            impulses: vec![Impulse::from_channels(rate, vec![raw::parse_text(text)?])],
            ..Default::default()
        }
    } else {
        return Err(ImportError::Failed(
            "Not EQ or a filter koan recognises: expected AutoEQ or Equalizer APO lines, CamillaDSP YAML, or coefficients".into(),
        ));
    };
    let name = if parsed.impulses.is_empty() {
        "Shared EQ"
    } else {
        "Shared filter"
    };
    Ok(Imported {
        name: name.into(),
        source: vec!["Shared text".into()],
        filters: parsed.filters,
        impulses: parsed.impulses,
    })
}

/// A file found, and whether it was named outright rather than found inside a
/// folder or zip. Something named outright that koan cannot read is an error;
/// a readme in a zip is not.
struct Found {
    path: PathBuf,
    named: bool,
}

fn gather(path: &Path, scratch: &Scratch, out: &mut Vec<Found>, named: bool) -> Result<(), String> {
    let hidden = path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with('.') || n == "__MACOSX");
    if hidden && !named {
        return Ok(());
    }
    if path.is_dir() {
        let mut entries: Vec<_> = std::fs::read_dir(path)
            .map_err(|e| format!("{}: {e}", path.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .collect();
        entries.sort();
        for entry in entries {
            gather(&entry, scratch, out, false)?;
        }
    } else if extension(path).as_deref() == Some("zip") {
        let dir = scratch.unzip(path)?;
        gather(&dir, scratch, out, false)?;
    } else {
        out.push(Found {
            path: path.to_path_buf(),
            named,
        });
    }
    Ok(())
}

fn extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
}

fn sort(files: Vec<Found>, rate: Option<u32>) -> Result<Imported, ImportError> {
    let mut imported = Imported::default();
    let mut configured = false;
    let mut loose_audio = Vec::new();
    let mut loose_raw = Vec::new();
    let mut eq_files: Vec<(PathBuf, Vec<DspFilter>)> = Vec::new();

    for Found { path, named } in files {
        let ext = extension(&path);
        match ext.as_deref() {
            Some("cfg") => {
                imported.impulses.extend(convolver::read(&path)?);
                configured = true;
            }
            Some("yml" | "yaml") => {
                let parsed = camilla::read(&path)?;
                configured |= !parsed.impulses.is_empty();
                imported.filters.extend(parsed.filters);
                imported.impulses.extend(parsed.impulses);
            }
            Some(e) if AUDIO.contains(&e) => loose_audio.push(path),
            Some("pcm") => loose_raw.push((path, RawFormat::F32Le)),
            Some("dbl") => loose_raw.push((path, RawFormat::F64Le)),
            Some("raw" | "bin") => loose_raw.push((path, RawFormat::F32Le)),
            _ => {
                let text = match std::fs::read_to_string(&path) {
                    Ok(t) => t,
                    Err(_) if !named => continue,
                    Err(e) => return Err(format!("{}: {e}", path.display()).into()),
                };
                if apo::looks_like(&text) {
                    let parsed = apo::read(&path)?;
                    configured |= !parsed.impulses.is_empty();
                    eq_files.push((path, parsed.filters));
                    imported.impulses.extend(parsed.impulses);
                } else if camilla::looks_like(&text) {
                    let parsed = camilla::read(&path)?;
                    configured |= !parsed.impulses.is_empty();
                    imported.filters.extend(parsed.filters);
                    imported.impulses.extend(parsed.impulses);
                } else if raw::looks_like_coefficients(&text) {
                    loose_raw.push((path, RawFormat::Text));
                } else if named {
                    return Err(format!("{}: not a filter or EQ koan reads", path.display()).into());
                }
            }
        }
    }

    // REW writes a file of bands per speaker. Several, each naming a side and
    // none choosing channels itself, are each that side's.
    let per_side = eq_files.len() > 1
        && eq_files.iter().all(|(path, filters)| {
            side_in_name(path).is_some()
                && filters
                    .iter()
                    .all(|f| !matches!(f, DspFilter::Mix(_)) && f.channels().is_empty())
        });
    for (path, mut filters) in eq_files {
        if per_side {
            let side = side_in_name(&path).expect("checked above") as u16;
            for c in filters.iter_mut().filter_map(DspFilter::channels_mut) {
                *c = vec![side];
            }
        }
        imported.filters.extend(filters);
    }

    // A configuration names its responses; loose ones beside it are those.
    if !configured {
        imported
            .impulses
            .extend(loose(loose_audio, loose_raw, rate)?);
    }
    if imported.filters.is_empty() && imported.impulses.is_empty() {
        return Err(ImportError::Failed(
            "Nothing to import: no EQ or filters found".into(),
        ));
    }
    Ok(imported)
}

/// A response file and its channels.
type Loaded = (PathBuf, Vec<Vec<f32>>);

/// Responses with no configuration naming them: each at its own rate, or the
/// one in its name, or `rate`.
fn loose(
    audio: Vec<PathBuf>,
    raw_files: Vec<(PathBuf, RawFormat)>,
    rate: Option<u32>,
) -> Result<Vec<Impulse>, ImportError> {
    let mut by_rate: BTreeMap<u32, Vec<Loaded>> = BTreeMap::new();
    for path in audio {
        let (r, channels) = read_audio(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        by_rate.entry(r).or_default().push((path, channels));
    }
    let mut unrated = Vec::new();
    for (path, format) in raw_files {
        let Some(r) = rate_in_name(&path).or(rate) else {
            unrated.push(file_name(&path));
            continue;
        };
        let samples =
            raw::read(&path, format, 0, 0).map_err(|e| format!("{}: {e}", path.display()))?;
        by_rate.entry(r).or_default().push((path, vec![samples]));
    }
    if !unrated.is_empty() {
        return Err(ImportError::NeedsRate(unrated.join(", ")));
    }

    by_rate
        .into_iter()
        .map(|(rate, mut files)| {
            if files.len() == 1 {
                let (_, channels) = files.remove(0);
                return Ok(Impulse::from_channels(rate, channels));
            }
            // Mono files for each side, as REW and rePhase export them.
            if files.iter().any(|(_, c)| c.len() != 1) {
                return Err(ImportError::Failed(format!(
                    "Several files at {rate} Hz and no .cfg to say which is for which channel"
                )));
            }
            let mut sides: [Option<Vec<f32>>; 2] = [None, None];
            for (path, mut channels) in files {
                let side = side_in_name(&path).ok_or_else(|| {
                    ImportError::Failed(format!(
                        "Can't tell which channel {} is for: name it L or R, or add a .cfg",
                        file_name(&path)
                    ))
                })?;
                if sides[side].replace(channels.remove(0)).is_some() {
                    return Err(ImportError::Failed(format!(
                        "Two {} files at {rate} Hz",
                        ["left", "right"][side]
                    )));
                }
            }
            match sides {
                [Some(l), Some(r)] => Ok(Impulse::from_channels(rate, vec![l, r])),
                _ => Err(ImportError::Failed(format!(
                    "Only one side's response at {rate} Hz"
                ))),
            }
        })
        .collect()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// A file's name split into runs of letters and runs of digits:
/// `room_L44.1k` → `room`, `l`, `44.1`, `k`.
fn tokens(path: &Path) -> Vec<String> {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let mut out: Vec<String> = Vec::new();
    let mut last: Option<bool> = None;
    for c in stem.chars() {
        let digit = c.is_ascii_digit() || (c == '.' && last == Some(true));
        if !(c.is_ascii_alphanumeric() || digit) {
            last = None;
            continue;
        }
        if last == Some(digit) {
            out.last_mut().expect("a run is open").push(c);
        } else {
            out.push(c.to_string());
        }
        last = Some(digit);
    }
    out
}

/// `44100`, `44.1k`, `44k`, `48`, `96khz`… in a file's name.
fn rate_in_name(path: &Path) -> Option<u32> {
    tokens(path).iter().rev().find_map(|t| match t.as_str() {
        "44100" | "44.1" | "441" | "44" => Some(44100),
        "48000" | "48" => Some(48000),
        "88200" | "88.2" | "882" | "88" => Some(88200),
        "96000" | "96" => Some(96000),
        "176400" | "176.4" | "1764" | "176" => Some(176400),
        "192000" | "192" => Some(192000),
        "352800" | "352.8" | "352" => Some(352800),
        "384000" | "384" => Some(384000),
        _ => None,
    })
}

/// 0 for left, 1 for right.
fn side_in_name(path: &Path) -> Option<usize> {
    let t = tokens(path);
    let left = t.iter().any(|t| t == "l" || t == "left");
    let right = t.iter().any(|t| t == "r" || t == "right");
    match (left, right) {
        (true, false) => Some(0),
        (false, true) => Some(1),
        _ => None,
    }
}

/// `Sennheiser HD 600 ParametricEQ.txt` → `Sennheiser HD 600`, and squig.link's
/// `Moondrop Aria Filters.txt` → `Moondrop Aria`.
fn profile_name(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = stem
        .trim_end_matches("ParametricEQ")
        .trim_end_matches(" Filters")
        .trim_end_matches(['_', '-', ' '])
        .to_string();
    if name.is_empty() {
        "Imported".into()
    } else {
        name
    }
}

/// Where zips are unpacked, removed when the import is done with it.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Self, String> {
        let dir = std::env::temp_dir().join(format!(
            "koan-dsp-import-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        Ok(Self(dir))
    }

    fn unzip(&self, path: &Path) -> Result<PathBuf, String> {
        let fail = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
        let file = std::fs::File::open(path).map_err(|e| fail(&e))?;
        let mut archive = zip::ZipArchive::new(file).map_err(|e| fail(&e))?;
        let dir = self.0.join(uuid::Uuid::now_v7().to_string());
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(|e| fail(&e))?;
            // `enclosed_name` refuses `..` and absolute paths.
            let Some(rel) = entry.enclosed_name() else {
                continue;
            };
            let dest = dir.join(rel);
            if entry.is_dir() {
                std::fs::create_dir_all(&dest).map_err(|e| fail(&e))?;
                continue;
            }
            // A filter is a few megabytes; anything near this is not one.
            if entry.size() > 512 << 20 {
                return Err(fail(&"an entry is too large to be a filter"));
            }
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(|e| fail(&e))?;
            }
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).map_err(|e| fail(&e))?;
            std::fs::write(&dest, bytes).map_err(|e| fail(&e))?;
        }
        Ok(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::dsp::raw::write_wav;
    use std::io::Write as _;

    #[test]
    fn rates_and_sides_in_names() {
        let p = |s: &str| PathBuf::from(s);
        assert_eq!(rate_in_name(&p("room-44.1k.txt")), Some(44100));
        assert_eq!(rate_in_name(&p("Cor1S192.wav")), Some(192000));
        assert_eq!(rate_in_name(&p("filter_96000_L.dbl")), Some(96000));
        assert_eq!(rate_in_name(&p("room.txt")), None);
        assert_eq!(side_in_name(&p("L44.wav")), Some(0));
        assert_eq!(side_in_name(&p("room_right_48k.wav")), Some(1));
        assert_eq!(side_in_name(&p("stereo.wav")), None);
        assert_eq!(
            profile_name(&p("Sennheiser HD 600 ParametricEQ.txt")),
            "Sennheiser HD 600"
        );
        assert_eq!(
            profile_name(&p("Moondrop Aria Filters.txt")),
            "Moondrop Aria"
        );
    }

    #[test]
    fn a_zip_of_mono_sides_at_two_rates() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("Living room.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, rate, v) in [
            ("L44.wav", 44100, 0.5),
            ("R44.wav", 44100, 0.25),
            ("L48.wav", 48000, 0.5),
            ("R48.wav", 48000, 0.25),
        ] {
            let tmp = dir.path().join(name);
            write_wav(&tmp, rate, &[vec![v]]).unwrap();
            zip.start_file(format!("filters/{name}"), opts).unwrap();
            zip.write_all(&std::fs::read(&tmp).unwrap()).unwrap();
        }
        zip.start_file("__MACOSX/._L44.wav", opts).unwrap();
        zip.write_all(b"junk").unwrap();
        zip.start_file("readme.pdf", opts).unwrap();
        zip.write_all(b"%PDF").unwrap();
        zip.finish().unwrap();

        let imported = import(&[zip_path], None).unwrap();
        assert_eq!(imported.name, "Living room");
        assert_eq!(imported.impulses.len(), 2);
        for ir in &imported.impulses {
            assert_eq!(ir.as_channels().unwrap(), vec![&[0.5][..], &[0.25][..]]);
        }
    }

    #[test]
    fn coefficients_need_a_rate_from_somewhere() {
        let dir = tempfile::tempdir().unwrap();
        let text: String = (0..32)
            .map(|i| format!("{}\n", if i == 0 { 1.0 } else { 0.0 }))
            .collect();
        let bare = dir.path().join("room.txt");
        std::fs::write(&bare, &text).unwrap();
        assert!(matches!(
            import(std::slice::from_ref(&bare), None),
            Err(ImportError::NeedsRate(_))
        ));
        assert_eq!(
            import(&[bare], Some(48000)).unwrap().impulses[0].rate,
            48000
        );

        let named = dir.path().join("room 96k.txt");
        std::fs::write(&named, &text).unwrap();
        assert_eq!(import(&[named], None).unwrap().impulses[0].rate, 96000);
    }

    #[test]
    fn rew_bands_for_each_speaker_land_on_its_channel() {
        let dir = tempfile::tempdir().unwrap();
        let l = dir.path().join("Room Left.txt");
        let r = dir.path().join("Room Right.txt");
        std::fs::write(&l, "Filter 1: ON PK Fc 50 Hz Gain -6 dB Q 4\n").unwrap();
        std::fs::write(&r, "Filter 1: ON PK Fc 60 Hz Gain -3 dB Q 4\n").unwrap();
        let imported = import(&[l, r], None).unwrap();
        assert_eq!(imported.filters[0].channels(), [0]);
        assert_eq!(imported.filters[1].channels(), [1]);
    }

    #[test]
    fn shared_text_is_read_as_what_it_is() {
        let eq = import_text(
            "Preamp: -3 dB\nFilter 1: ON PK Fc 100 Hz Gain 3 dB Q 1\n",
            None,
        )
        .unwrap();
        assert_eq!(eq.filters.len(), 1);
        assert!(import_text("hello there", None).is_err());
    }
}
