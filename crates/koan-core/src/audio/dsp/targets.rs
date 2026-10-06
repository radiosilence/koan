//! Headphone target curves, and moving an AutoEQ correction from the target it
//! was made for to another.
//!
//! An AutoEQ result corrects a headphone to one target, on the rig it was
//! measured on. Its CSV carries that target, rig compensation and all. Moving
//! it to another target adds one curve after the correction: the difference
//! between the target chosen and the one the result was made for. Both are
//! taken from the same reference set, the files in `targets/` (see
//! `SOURCES.md`), so whatever compensation the result's rig carries is common
//! to both and cancels. The result's own target only says which of the set it
//! was made for, and when it matches none of them closely, no other target is
//! offered: a difference taken across rigs would correct the rig, not the
//! taste.
//!
//! The curve runs as a `graphic` filter, which `steps` makes a
//! minimum-phase FIR at the output rate.

use std::path::{Path, PathBuf};

use crate::config::{self, GraphicEq};

/// Which kind of headphone a target is for. A target for one is not offered
/// for the other: the measurements behind them differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ear {
    Over,
    In,
}

/// A target that ships with koan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    pub id: &'static str,
    pub name: &'static str,
    /// What it sounds like, in a line.
    pub character: &'static str,
    pub ear: Ear,
    data: &'static str,
}

/// The targets offered, over-ear then in-ear, the most chosen first.
pub const TARGETS: &[Target] = &[
    Target {
        id: "harman-over-ear-2018",
        name: "Harman over-ear 2018",
        character: "What most listeners in Harman's research preferred: a warm bass shelf, a forward upper midrange and a soft top end.",
        ear: Ear::Over,
        data: include_str!("targets/harman-over-ear-2018.csv"),
    },
    Target {
        id: "harman-over-ear-2018-without-bass",
        name: "Harman over-ear 2018, no bass shelf",
        character: "Harman's curve with a flat low end: leaner bass, the same mids and treble.",
        ear: Ear::Over,
        data: include_str!("targets/harman-over-ear-2018-without-bass.csv"),
    },
    Target {
        id: "oratory1990-over-ear",
        name: "oratory1990 over-ear",
        character: "oratory1990's target for over-ears, close to Harman's.",
        ear: Ear::Over,
        data: include_str!("targets/oratory1990-over-ear.csv"),
    },
    Target {
        id: "diffuse-field-gras-kemar",
        name: "Diffuse field",
        character: "Even sound from every direction, as a room without reflections would give: no bass shelf, and brighter than Harman.",
        ear: Ear::Over,
        data: include_str!("targets/diffuse-field-gras-kemar.csv"),
    },
    Target {
        id: "harman-in-ear-2019",
        name: "Harman in-ear 2019",
        character: "Harman's in-ear preference target: a bigger bass shelf than over-ear, and more treble.",
        ear: Ear::In,
        data: include_str!("targets/harman-in-ear-2019.csv"),
    },
    Target {
        id: "harman-in-ear-2019-without-bass",
        name: "Harman in-ear 2019, no bass shelf",
        character: "Harman's in-ear curve with a flat low end.",
        ear: Ear::In,
        data: include_str!("targets/harman-in-ear-2019-without-bass.csv"),
    },
    Target {
        id: "autoeq-in-ear",
        name: "AutoEQ in-ear",
        character: "AutoEQ's own in-ear target.",
        ear: Ear::In,
        data: include_str!("targets/autoeq-in-ear.csv"),
    },
    Target {
        id: "oratory1990-in-ear",
        name: "oratory1990 in-ear",
        character: "oratory1990's target for in-ears.",
        ear: Ear::In,
        data: include_str!("targets/oratory1990-in-ear.csv"),
    },
];

/// A result's own target is taken for one of `TARGETS` when it is within this
/// much of it, RMS over the whole band, both levelled at 1 kHz. A result made
/// for one of them on another rig stays within a decibel or so; one made for
/// something else is well past this.
const SAME_TARGET_DB: f64 = 1.5;

/// The furthest a target difference moves any frequency.
const MAX_DB: f64 = 12.0;

/// How much nearer the best match must be than the next for a result to be
/// taken for it. Shipped targets differ by less than a decibel in places,
/// and a guess between two would apply the wrong difference.
const MARGIN_DB: f64 = 0.3;

/// The level a curve's point may have. Targets sit within about ±15 dB; a
/// value past this is a file that is not one, and is refused rather than
/// carried into a filter.
const LEVEL_LIMIT_DB: f64 = 40.0;

/// The largest file read as a target.
const FILE_CAP: u64 = 1 << 20;

impl Ear {
    /// The kind of headphone an AutoEQ result is for, from its path in the
    /// results: `…/over-ear/…`, `…/711 in-ear/…`, `…/earbud/…`.
    pub fn of_result_path(path: &str) -> Option<Self> {
        let lower = path.to_ascii_lowercase();
        if lower.contains("in-ear") || lower.contains("earbud") {
            Some(Ear::In)
        } else if lower.contains("over-ear") || lower.contains("on-ear") {
            Some(Ear::Over)
        } else {
            None
        }
    }
}

/// A curve: (Hz, dB), rising in frequency.
pub type Curve = Vec<(f64, f64)>;

/// Read a curve: two numbers per line, frequency and level, separated by a
/// comma, a tab or spaces, as AutoEQ's CSVs, REW exports and squig.link's
/// text have them. Header and comment lines are skipped, and so is a point
/// that is not a frequency in hertz with a level within ±40 dB.
pub fn parse(text: &str) -> Curve {
    let mut curve: Curve = text
        .lines()
        .filter_map(|line| {
            let mut fields = line
                .split(|c: char| c == ',' || c == ';' || c.is_whitespace())
                .filter(|f| !f.is_empty());
            let hz: f64 = fields.next()?.parse().ok()?;
            let db: f64 = fields.next()?.parse().ok()?;
            (hz > 0.0 && hz.is_finite() && db.is_finite() && db.abs() <= LEVEL_LIMIT_DB)
                .then_some((hz, db))
        })
        .collect();
    curve.sort_by(|a, b| a.0.total_cmp(&b.0));
    curve.dedup_by(|a, b| a.0 == b.0);
    curve
}

impl Target {
    pub fn curve(&self) -> Curve {
        parse(self.data)
    }
}

pub fn shipped(id: &str) -> Option<&'static Target> {
    TARGETS.iter().find(|t| t.id == id)
}

/// The level of `curve` at `hz`, interpolated against log frequency and held
/// past either end.
fn at(curve: &[(f64, f64)], hz: f64) -> f64 {
    let (Some(first), Some(last)) = (curve.first(), curve.last()) else {
        return 0.0;
    };
    if hz <= first.0 {
        return first.1;
    }
    if hz >= last.0 {
        return last.1;
    }
    let i = curve.partition_point(|p| p.0 <= hz);
    let ((f0, g0), (f1, g1)) = (curve[i - 1], curve[i]);
    g0 + (g1 - g0) * (hz / f0).ln() / (f1 / f0).ln()
}

/// AutoEQ's grid: what the shipped targets and every result use.
fn grid() -> Vec<f64> {
    TARGETS[0].curve().into_iter().map(|(hz, _)| hz).collect()
}

/// `curve` on `grid`, levelled to 0 dB at 1 kHz.
fn levelled(curve: &[(f64, f64)], grid: &[f64]) -> Vec<f64> {
    let k = at(curve, 1000.0);
    grid.iter().map(|&hz| at(curve, hz) - k).collect()
}

/// Which of `TARGETS` for `ear` a result was made for, from the target its
/// CSV carries: the nearest, when it is close and clearly nearer than the
/// next. Targets for the other kind of headphone are not compared, since
/// some lie within a decibel of each other.
pub fn identify(result_target: &[(f64, f64)], ear: Ear) -> Option<&'static Target> {
    if result_target.len() < 2
        || result_target
            .iter()
            .any(|(f, d)| !f.is_finite() || !d.is_finite())
    {
        return None;
    }
    let grid = grid();
    let theirs = levelled(result_target, &grid);
    let rms = |t: &Target| {
        let ours = levelled(&t.curve(), &grid);
        let sum: f64 = theirs.iter().zip(&ours).map(|(a, b)| (a - b).powi(2)).sum();
        (sum / grid.len() as f64).sqrt()
    };
    let mut near: Vec<(f64, &'static Target)> = TARGETS
        .iter()
        .filter(|t| t.ear == ear)
        .map(|t| (rms(t), t))
        .collect();
    near.sort_by(|a, b| a.0.total_cmp(&b.0));
    match near.as_slice() {
        [(best, t), rest @ ..]
            if *best <= SAME_TARGET_DB
                && rest
                    .first()
                    .is_none_or(|(next, _)| next - best >= MARGIN_DB) =>
        {
            Some(t)
        }
        _ => None,
    }
}

/// The curve that moves a correction made for `from` to `to`: their
/// difference, levelled at 1 kHz, smoothed over a twelfth of an octave so the
/// FIR follows the shape rather than each wiggle, held within ±12 dB, and
/// sampled every twelfth of an octave, which the graphic filter interpolates
/// between.
pub fn difference(from: &[(f64, f64)], to: &[(f64, f64)]) -> GraphicEq {
    let grid = grid();
    let (a, b) = (levelled(from, &grid), levelled(to, &grid));
    // A curve that is not a target's makes no difference at all, rather than
    // one that is not a number.
    let delta: Vec<f64> = b
        .iter()
        .zip(&a)
        .map(|(t, f)| t - f)
        .map(|d| if d.is_finite() { d } else { 0.0 })
        .collect();
    let smoothed: Vec<f64> = grid
        .iter()
        .map(|&hz| {
            let (lo, hi) = (hz / 2f64.powf(1.0 / 24.0), hz * 2f64.powf(1.0 / 24.0));
            let near: Vec<f64> = grid
                .iter()
                .zip(&delta)
                .filter(|(f, _)| (lo..=hi).contains(*f))
                .map(|(_, d)| *d)
                .collect();
            near.iter().sum::<f64>() / near.len().max(1) as f64
        })
        .collect();
    let mut points = Vec::new();
    let mut next = 20.0;
    for (&hz, &db) in grid.iter().zip(&smoothed) {
        if hz >= next || Some(&hz) == grid.last() {
            let db = if db.is_nan() { 0.0 } else { db };
            points.push((hz, db.clamp(-MAX_DB, MAX_DB)));
            next = hz * 2f64.powf(1.0 / 12.0);
        }
    }
    GraphicEq {
        points,
        channels: Vec::new(),
    }
}

// --- Targets a person adds ---------------------------------------------------

/// Where added targets are kept, one CSV each.
fn added_dir() -> PathBuf {
    config::config_dir().join("dsp").join("targets")
}

/// An id for an added target, which `choice_curve` reads back.
const ADDED: &str = "added:";

/// A target a person added: its id and name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Added {
    pub id: String,
    pub name: String,
}

/// The targets a person has added, by name.
pub fn added() -> Vec<Added> {
    let Ok(dir) = std::fs::read_dir(added_dir()) else {
        return Vec::new();
    };
    let mut out: Vec<Added> = dir
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.path().file_stem()?.to_string_lossy().into_owned();
            (e.path().extension()? == "csv").then(|| Added {
                id: format!("{ADDED}{name}"),
                name,
            })
        })
        .collect();
    out.sort_by_key(|a| a.name.to_lowercase());
    out
}

/// Add a target from a file: a CSV of frequency and level, or a squig.link
/// export. Kept on AutoEQ's grid, named after the file. Refused unless it
/// covers the audible band well enough to take a difference from: points
/// from below 100 Hz to above 10 kHz, at least twenty of them.
pub fn add(path: &Path) -> Result<Added, String> {
    use std::io::Read as _;
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(FILE_CAP + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() as u64 > FILE_CAP {
        return Err("A target file is a few kilobytes; this one is over a megabyte".into());
    }
    let text = String::from_utf8_lossy(&bytes);
    let curve = parse(&text);
    let (Some(first), Some(last)) = (curve.first(), curve.last()) else {
        return Err("No frequency and level pairs in it".into());
    };
    if curve.len() < 20 || first.0 > 100.0 || last.0 < 10_000.0 {
        return Err(
            "A target needs points from below 100 Hz to above 10 kHz, at least twenty of them"
                .into(),
        );
    }
    let name: String = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Target".into())
        .chars()
        .filter(|c| !matches!(c, '/' | '\\' | ':'))
        .collect();
    let name = name.trim().to_owned();
    if name.is_empty() || name.starts_with('.') {
        return Err("The file needs a name to call the target by".into());
    }
    let mut out = String::from("frequency,raw\n");
    for hz in grid() {
        out.push_str(&format!("{hz:.2},{:.2}\n", at(&curve, hz)));
    }
    let dir = added_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(format!("{name}.csv")), out).map_err(|e| e.to_string())?;
    Ok(Added {
        id: format!("{ADDED}{name}"),
        name,
    })
}

/// The curve a chosen target id names: one that ships, or one added.
pub fn choice_curve(id: &str) -> Option<Curve> {
    if let Some(t) = shipped(id) {
        return Some(t.curve());
    }
    let name = id.strip_prefix(ADDED)?;
    if name.contains(['/', '\\']) || name.starts_with('.') {
        return None;
    }
    let text = std::fs::read_to_string(added_dir().join(format!("{name}.csv"))).ok()?;
    Some(parse(&text)).filter(|c| !c.is_empty())
}

/// Where a profile installed from AutoEQ keeps its result's CSV.
pub fn result_path(dsp_dir: &Path) -> PathBuf {
    dsp_dir.join("autoeq.csv")
}

/// A column of an AutoEQ result CSV, as a curve.
pub fn result_column(text: &str, column: &str) -> Curve {
    let mut lines = text.lines();
    let Some(header) = lines.next() else {
        return Vec::new();
    };
    let names: Vec<&str> = header.split(',').map(str::trim).collect();
    let (Some(f), Some(c)) = (
        names.iter().position(|n| *n == "frequency"),
        names.iter().position(|n| *n == column),
    ) else {
        return Vec::new();
    };
    lines
        .filter_map(|line| {
            let fields: Vec<&str> = line.split(',').collect();
            let hz: f64 = fields.get(f)?.trim().parse().ok()?;
            let db: f64 = fields.get(c)?.trim().parse().ok()?;
            Some((hz, db))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_target_is_on_autoeqs_grid() {
        let grid = grid();
        assert_eq!(grid.len(), 695);
        assert_eq!(grid[0], 20.0);
        assert!(*grid.last().unwrap() > 19_900.0);
        for t in TARGETS {
            let c = t.curve();
            assert_eq!(c.len(), 695, "{}", t.id);
            assert!(c.iter().zip(&grid).all(|((f, _), g)| f == g), "{}", t.id);
        }
    }

    #[test]
    fn a_result_is_known_by_its_target_and_its_rigs_offset_is_tolerated() {
        let harman = shipped("harman-over-ear-2018").unwrap();
        assert_eq!(
            identify(&harman.curve(), Ear::Over).map(|t| t.id),
            Some(harman.id)
        );
        // A rig's compensation: a dip of a few dB around 6 kHz, as AutoEQ's
        // Rtings and Innerfidelity results carry.
        let rig: Curve = harman
            .curve()
            .into_iter()
            .map(|(hz, db)| {
                let x = (hz / 6000.0).log2();
                (hz, db - 3.0 * (-x * x * 4.0).exp())
            })
            .collect();
        assert_eq!(identify(&rig, Ear::Over).map(|t| t.id), Some(harman.id));
        let no_bass = shipped("harman-over-ear-2018-without-bass").unwrap();
        assert_eq!(
            identify(&no_bass.curve(), Ear::Over).map(|t| t.id),
            Some(no_bass.id)
        );
        // Something none of them is near.
        let tilted: Curve = harman
            .curve()
            .into_iter()
            .map(|(hz, db)| (hz, db + 6.0 * (hz / 1000.0).log2()))
            .collect();
        assert_eq!(identify(&tilted, Ear::Over), None);
    }

    /// An in-ear result is never taken for an over-ear target however near
    /// one lies, and a result between two targets is taken for neither.
    #[test]
    fn identification_keeps_to_the_ear_and_refuses_to_guess() {
        let in_ear = shipped("harman-in-ear-2019").unwrap();
        assert_eq!(
            identify(&in_ear.curve(), Ear::In).map(|t| t.id),
            Some(in_ear.id)
        );
        // AutoEQ's in-ear target lies within a decibel of Harman over-ear
        // 2018 without its bass shelf; as an in-ear result it is itself.
        let autoeq = shipped("autoeq-in-ear").unwrap();
        assert_eq!(
            identify(&autoeq.curve(), Ear::In).map(|t| t.id),
            Some(autoeq.id)
        );
        // Halfway between Harman 2018 with and without its bass shelf.
        let with = shipped("harman-over-ear-2018").unwrap().curve();
        let without = shipped("harman-over-ear-2018-without-bass")
            .unwrap()
            .curve();
        let between: Curve = with
            .iter()
            .zip(&without)
            .map(|((f, a), (_, b))| (*f, (a + b) / 2.0))
            .collect();
        assert_eq!(identify(&between, Ear::Over), None);
        assert_eq!(
            Ear::of_result_path("crinacle/711 in-ear/1Custom SA02"),
            Some(Ear::In)
        );
        assert_eq!(
            Ear::of_result_path("oratory1990/over-ear/Sennheiser HD 650"),
            Some(Ear::Over)
        );
        assert_eq!(
            Ear::of_result_path("Rtings/HMS II.3 over-ear/X"),
            Some(Ear::Over)
        );
        assert_eq!(Ear::of_result_path("someone/elsewhere/X"), None);
    }

    /// A file of garbage cannot put anything but numbers into a filter.
    #[test]
    fn a_hostile_curve_cannot_make_a_filter_that_is_not_a_number() {
        let hostile = "20000,-3\n20,nan\n100,inf\n200,-inf\n300,1.7e308\n400,-1.7e308\n500,-50\n1000,0\n50,2\n";
        let curve = parse(hostile);
        assert_eq!(curve, vec![(50.0, 2.0), (1000.0, 0.0), (20000.0, -3.0)]);
        let from = shipped("harman-over-ear-2018").unwrap().curve();
        let g = difference(
            &from,
            &[(20.0, f64::MAX), (21.0, -f64::MAX), (20000.0, 0.0)],
        );
        assert!(g.points.iter().all(|(f, d)| f.is_finite() && d.is_finite()));
    }

    #[test]
    fn the_difference_is_the_targets_apart_and_keeps_the_rigs_part() {
        let from = shipped("harman-over-ear-2018").unwrap().curve();
        let to = shipped("harman-over-ear-2018-without-bass")
            .unwrap()
            .curve();
        let g = difference(&from, &to);
        let level = |hz: f64| at(&g.points, hz);
        assert!(level(1000.0).abs() < 0.2, "level at 1 kHz");
        assert!(
            level(30.0) < -3.0,
            "the bass shelf comes off: {}",
            level(30.0)
        );
        assert!(level(4000.0).abs() < 0.5, "the rest is alike");
        // Points every twelfth of an octave from 20 Hz: about 120.
        assert!((110..=125).contains(&g.points.len()), "{}", g.points.len());
        assert!(g.points.iter().all(|(_, db)| db.abs() <= MAX_DB));
        // The same target: no difference.
        assert!(
            difference(&from, &from)
                .points
                .iter()
                .all(|(_, db)| db.abs() < 1e-9)
        );
    }

    #[test]
    fn curves_are_read_from_csv_or_squiglink_text() {
        let csv = "frequency,raw\n20,1.5\n1000,0\n20000,-3\n";
        assert_eq!(
            parse(csv),
            vec![(20.0, 1.5), (1000.0, 0.0), (20000.0, -3.0)]
        );
        let squig = "* squig.link target\n20\t1.5\n1000\t0\n20000 -3\n";
        assert_eq!(parse(squig), parse(csv));
        let result = "frequency,raw,target\n20,-6,3.3\n1000,0,0\n";
        assert_eq!(
            result_column(result, "target"),
            vec![(20.0, 3.3), (1000.0, 0.0)]
        );
    }

    #[test]
    fn an_added_target_is_kept_on_the_grid_and_read_back() {
        let _guard = crate::config::tests::PERSIST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        config::set_config_dir(dir.path());
        let file = dir.path().join("Super Warm.txt");
        let mut text = String::from("Frequency\tdB\n");
        for i in 0..40 {
            let hz = 20.0 * 2f64.powf(i as f64 / 4.0);
            text.push_str(&format!("{hz}\t{}\n", 10.0 - i as f64 / 4.0));
        }
        std::fs::write(&file, text).unwrap();
        let added = add(&file).unwrap();
        assert_eq!(added.name, "Super Warm");
        assert_eq!(super::added(), vec![added.clone()]);
        let curve = choice_curve(&added.id).unwrap();
        assert_eq!(curve.len(), 695);
        assert!(choice_curve("added:../escape").is_none());

        let short = dir.path().join("short.csv");
        std::fs::write(&short, "100,0\n1000,0\n").unwrap();
        assert!(add(&short).is_err());
    }

    /// A correction moved to another target plays their difference after
    /// its own filters; moved back, or never moved, nothing is added.
    #[test]
    fn a_chosen_target_adds_one_curve_to_the_chain() {
        use crate::audio::dsp::profiles;
        use crate::config::{Config, DspFilter, DspProfile, DspTarget, EqFilter, EqFilterKind};
        let _guard = crate::config::tests::PERSIST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        config::set_config_dir(dir.path());
        let band = DspFilter::Band(EqFilter {
            kind: EqFilterKind::Peaking,
            freq: 100.0,
            gain_db: 3.0,
            q: 1.0,
            channels: vec![],
        });
        Config::persist(|c| {
            c.dsp.profiles.push(DspProfile {
                name: "HD 650 (AutoEQ, oratory1990)".into(),
                filters: vec![band.clone()],
                target: Some(DspTarget {
                    made_for: "harman-over-ear-2018".into(),
                    chosen: None,
                }),
                ..Default::default()
            })
        })
        .unwrap();
        let name = "HD 650 (AutoEQ, oratory1990)";
        let loaded = || {
            let cfg = Config::cached();
            let p = cfg
                .dsp
                .profiles
                .iter()
                .find(|p| p.name == name)
                .unwrap()
                .clone();
            super::super::Setup::load(&p, dir.path())
                .unwrap()
                .unwrap()
                .filters
        };
        assert_eq!(loaded(), vec![band.clone()]);

        let choices = profiles::target_choices(name).unwrap();
        assert_eq!(choices.made_for.id, "harman-over-ear-2018");
        assert!(
            choices
                .choices
                .iter()
                .all(|c| shipped(&c.id).is_none_or(|t| t.ear == Ear::Over))
        );
        assert!(
            profiles::choose_target(name, Some("harman-in-ear-2019")).is_err(),
            "in-ear"
        );
        profiles::choose_target(name, Some("harman-over-ear-2018-without-bass")).unwrap();
        let filters = loaded();
        assert_eq!(filters.len(), 2);
        assert_eq!(filters[0], band);
        assert!(matches!(filters[1], DspFilter::Graphic(_)));

        // Its own target is no move at all.
        profiles::choose_target(name, Some("harman-over-ear-2018")).unwrap();
        assert_eq!(loaded(), vec![band.clone()]);
        profiles::choose_target(name, Some("diffuse-field-gras-kemar")).unwrap();
        profiles::choose_target(name, None).unwrap();
        assert_eq!(loaded(), vec![band]);
    }
}
