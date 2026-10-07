//! Loudspeaker measurements exported from a Klippel Near-Field Scanner, as
//! Audio Science Review publishes them: `SPL Horizontal.txt` and
//! `SPL Vertical.txt`, tab-separated, a title row, a row of angles
//! (`On-Axis`, `10°`, `-10°` … `180°`), a header row of `Frequency / Hz` and
//! `Sound Pressure Level / dB …` pairs, then the data, a pair of columns per
//! angle. Frequencies above 1 kHz carry a thousands separator (`19,999.5`).
//!
//! A text may hold both planes, one after the other. With both, the curve is
//! CTA-2034's listening window, which predicts a speaker in a room better
//! than its on-axis response; with one, the on-axis response.

use super::targets::{Curve, at};

/// Which curve a speaker's export was read as.
#[derive(Debug, Clone, PartialEq)]
pub enum Used {
    /// CTA-2034's listening window, from both planes.
    ListeningWindow,
    /// The on-axis response, from the one plane given, if its title names it.
    OnAxis(Option<Plane>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Plane {
    Horizontal,
    Vertical,
}

impl Plane {
    fn name(self) -> &'static str {
        match self {
            Plane::Horizontal => "SPL Horizontal",
            Plane::Vertical => "SPL Vertical",
        }
    }
}

impl Used {
    /// What the person is told the correction is made from.
    pub fn describe(&self) -> String {
        match self {
            Used::ListeningWindow => {
                "A speaker's listening window (CTA-2034), from its horizontal \
                 and vertical measurements: on-axis, ±10° vertical and ±10°, ±20° and ±30° \
                 horizontal, averaged in power."
                    .into()
            }
            Used::OnAxis(plane) => {
                let other = match plane {
                    Some(Plane::Horizontal) => Plane::Vertical.name(),
                    Some(Plane::Vertical) => Plane::Horizontal.name(),
                    None => "the other plane's",
                };
                format!(
                    "A speaker's on-axis response. Add its {other} export as well, and the \
                     correction is made from its listening window (CTA-2034) instead, which \
                     predicts how it sounds in a room better."
                )
            }
        }
    }
}

/// One plane's export: its title, if it names a plane, and a curve per angle.
struct Export {
    plane: Option<Plane>,
    angles: Vec<(f64, Curve)>,
}

/// Whether `text` is a Klippel export, by its header row.
pub fn is_export(text: &str) -> bool {
    text.lines().take(10).any(is_header)
}

fn is_header(line: &str) -> bool {
    let cells: Vec<&str> = cells(line).collect();
    cells.iter().any(|c| c.starts_with("Frequency"))
        && cells.iter().any(|c| c.starts_with("Sound Pressure Level"))
}

/// A row's cells, unquoted and trimmed.
fn cells(line: &str) -> impl Iterator<Item = &str> {
    line.split('\t').map(|c| c.trim().trim_matches('"').trim())
}

/// A number as the export writes one, thousands separator and all.
fn number(cell: &str) -> Option<f64> {
    let n: f64 = cell.replace(',', "").parse().ok()?;
    n.is_finite().then_some(n)
}

/// An angle cell: `On-Axis` is 0°, `-10°` is -10°.
fn angle(cell: &str) -> Option<f64> {
    if cell.eq_ignore_ascii_case("on-axis") {
        return Some(0.0);
    }
    number(cell.trim_end_matches('°'))
}

fn exports(text: &str) -> Vec<Export> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if !is_header(line) {
            continue;
        }
        // Every non-empty angle cell heads a pair of columns.
        let angles: Vec<Option<f64>> = i
            .checked_sub(1)
            .map(|a| {
                cells(lines[a])
                    .filter(|c| !c.is_empty())
                    .map(angle)
                    .collect()
            })
            .unwrap_or_default();
        let plane = i.checked_sub(2).and_then(|t| {
            let title = lines[t].to_ascii_lowercase();
            if title.contains("horizontal") {
                Some(Plane::Horizontal)
            } else if title.contains("vertical") {
                Some(Plane::Vertical)
            } else {
                None
            }
        });
        let mut curves: Vec<Curve> = vec![Vec::new(); angles.len().max(1)];
        for row in &lines[i + 1..] {
            let values: Vec<Option<f64>> = cells(row).map(number).collect();
            if values.first().copied().flatten().is_none() {
                break;
            }
            for (k, curve) in curves.iter_mut().enumerate() {
                if let (Some(Some(hz)), Some(Some(db))) = (values.get(2 * k), values.get(2 * k + 1))
                    && *hz > 0.0
                {
                    curve.push((*hz, *db));
                }
            }
        }
        let angles = if angles.is_empty() {
            // No angle row: the first pair is taken for on-axis.
            curves.into_iter().take(1).map(|c| (0.0, c)).collect()
        } else {
            angles
                .into_iter()
                .zip(curves)
                .filter_map(|(a, c)| Some((a?, c)))
                .filter(|(_, c)| !c.is_empty())
                .collect()
        };
        out.push(Export { plane, angles });
    }
    out
}

/// The curve to correct from in a Klippel export, in dB SPL, and which one
/// it is.
pub fn read(text: &str) -> Result<(Curve, Used), String> {
    let exports = exports(text);
    let find = |plane: Plane| exports.iter().find(|e| e.plane == Some(plane));
    let curve_at = |e: &Export, deg: f64| {
        e.angles
            .iter()
            .find(|(a, _)| (a - deg).abs() < 0.5)
            .map(|(_, c)| c.clone())
    };
    if let (Some(h), Some(v)) = (find(Plane::Horizontal), find(Plane::Vertical)) {
        let window: Option<Vec<Curve>> = [
            (h, 0.0),
            (v, 10.0),
            (v, -10.0),
            (h, 10.0),
            (h, -10.0),
            (h, 20.0),
            (h, -20.0),
            (h, 30.0),
            (h, -30.0),
        ]
        .into_iter()
        .map(|(e, deg)| curve_at(e, deg))
        .collect();
        if let Some(window) = window {
            return Ok((power_average(&window), Used::ListeningWindow));
        }
    }
    let first = exports.first().ok_or_else(|| {
        "It looks like a Klippel export, but has no data under its header row".to_owned()
    })?;
    let on_axis = curve_at(first, 0.0).ok_or_else(|| {
        "It looks like a Klippel export, but has no on-axis column to read".to_owned()
    })?;
    Ok((on_axis, Used::OnAxis(first.plane)))
}

/// `curves` averaged in power, at the first one's frequencies.
fn power_average(curves: &[Curve]) -> Curve {
    curves[0]
        .iter()
        .map(|&(hz, _)| {
            let power = curves
                .iter()
                .map(|c| 10f64.powf(at(c, hz) / 10.0))
                .sum::<f64>()
                / curves.len() as f64;
            (hz, 10.0 * power.log10())
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A synthetic export in Klippel's layout: `level(angle, hz)` at each of
    /// the scanner's angles, frequencies past 1 kHz written with a
    /// thousands separator.
    pub(crate) fn export(title: &str, level: impl Fn(f64, f64) -> f64) -> String {
        let angles: Vec<f64> = std::iter::once(0.0)
            .chain((1..=17).flat_map(|k| [10.0 * k as f64, -10.0 * k as f64]))
            .chain(std::iter::once(180.0))
            .collect();
        let label = |a: f64| {
            if a == 0.0 {
                "\"On-Axis\"".to_owned()
            } else {
                format!("\"{a}°\"")
            }
        };
        let mut out = format!("\"{title}\"{}\r\n", "\t".repeat(2 * angles.len() - 1));
        out.push_str(
            &angles
                .iter()
                .map(|&a| label(a))
                .collect::<Vec<_>>()
                .join("\t\t"),
        );
        out.push_str("\r\n");
        out.push_str(
            &angles
                .iter()
                .map(|_| {
                    "\"Frequency / Hz\"\t\"Sound Pressure Level / dB (re 20 µPa/V)  [2.83  V @ 1. m]\""
                })
                .collect::<Vec<_>>()
                .join("\t"),
        );
        out.push_str("\r\n");
        let mut hz: f64 = 20.5078;
        while hz < 20_000.0 {
            let mut freq = format!("{hz:.4}");
            let point = freq.find('.').unwrap();
            if point > 3 {
                freq.insert(point - 3, ',');
            }
            let row: Vec<String> = angles
                .iter()
                .map(|&a| format!("{freq}\t{:.4}", level(a, hz)))
                .collect();
            out.push_str(&row.join("\t"));
            out.push_str("\r\n");
            hz *= 1.0352;
        }
        out
    }

    #[test]
    fn thousands_separators_are_read() {
        assert_eq!(number("19,999.5"), Some(19_999.5));
        assert_eq!(number("20.5078"), Some(20.5078));
        assert_eq!(angle("On-Axis"), Some(0.0));
        assert_eq!(angle("-10°"), Some(-10.0));
    }

    #[test]
    fn one_plane_is_read_on_axis() {
        let text = export("SPL Vertical", |a, hz| {
            85.0 + if hz > 5000.0 { a / 10.0 } else { 0.0 }
        });
        assert!(is_export(&text));
        let (curve, used) = read(&text).unwrap();
        assert_eq!(used, Used::OnAxis(Some(Plane::Vertical)));
        assert!(
            curve.last().unwrap().0 > 19_000.0,
            "frequencies above 1 kHz read"
        );
        assert!(curve.iter().all(|&(_, db)| (db - 85.0).abs() < 1e-9));
    }

    /// Both planes give the listening window: the nine curves CTA-2034
    /// names, averaged in power, and nothing wider.
    #[test]
    fn both_planes_give_the_listening_window() {
        // Each angle a flat level of its own, so the average is known; the
        // angles past the window are loud enough to show if they leaked in.
        let level = |a: f64| {
            if a.abs() <= 30.0 {
                80.0 - a.abs() / 10.0
            } else {
                120.0
            }
        };
        let text = export("SPL Horizontal", |a, _| level(a))
            + &export("SPL Vertical", |a, _| {
                if a.abs() == 10.0 { 70.0 } else { level(a) }
            });
        let (curve, used) = read(&text).unwrap();
        assert_eq!(used, Used::ListeningWindow);
        let window = [80.0, 70.0, 70.0, 79.0, 79.0, 78.0, 78.0, 77.0, 77.0];
        let expected = 10.0
            * (window
                .iter()
                .map(|db: &f64| 10f64.powf(db / 10.0))
                .sum::<f64>()
                / 9.0)
                .log10();
        assert!(curve.iter().all(|&(_, db)| (db - expected).abs() < 1e-9));
    }
}
