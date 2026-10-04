//! AutoEQ's `ParametricEQ.txt`, which is Equalizer APO's configuration format:
//!
//! ```text
//! Preamp: -6.8 dB
//! Filter 1: ON PK Fc 20 Hz Gain -1.3 dB Q 2.000
//! Filter 2: ON LSC Fc 105 Hz Gain 5.5 dB Q 0.70
//! ```

use crate::config::{EqFilter, EqFilterKind};

#[derive(Debug, Default, PartialEq)]
pub struct Parsed {
    pub preamp_db: Option<f64>,
    pub filters: Vec<EqFilter>,
}

/// A filter type koan has no equivalent of is an error rather than skipped:
/// dropping a band changes the correction without saying so.
pub fn parse(text: &str) -> Result<Parsed, String> {
    let mut parsed = Parsed::default();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        let fail = |why: &str| format!("line {}: {why}: {line}", n + 1);
        if let Some(rest) = line.strip_prefix("Preamp:") {
            let db = rest.trim().trim_end_matches("dB").trim();
            parsed.preamp_db = Some(db.parse().map_err(|_| fail("bad preamp"))?);
            continue;
        }
        let Some((head, body)) = line.split_once(':') else {
            continue;
        };
        if !head.starts_with("Filter") {
            continue;
        }
        let words: Vec<&str> = body.split_whitespace().collect();
        match words.first() {
            Some(&"ON") => {}
            Some(&"OFF") => continue,
            _ => return Err(fail("expected ON or OFF")),
        }
        let kind = match words.get(1).copied() {
            Some("PK" | "PEQ" | "Modal") => EqFilterKind::Peaking,
            Some("LS" | "LSC") => EqFilterKind::LowShelf,
            Some("HS" | "HSC") => EqFilterKind::HighShelf,
            Some("LP" | "LPQ") => EqFilterKind::LowPass,
            Some("HP" | "HPQ") => EqFilterKind::HighPass,
            _ => return Err(fail("unsupported filter type")),
        };
        let value = |key: &str| -> Result<Option<f64>, String> {
            match words.iter().position(|w| *w == key) {
                Some(i) => words
                    .get(i + 1)
                    .and_then(|v| v.parse().ok())
                    .map(Some)
                    .ok_or_else(|| fail(&format!("bad {key}"))),
                None => Ok(None),
            }
        };
        parsed.filters.push(EqFilter {
            kind,
            freq: value("Fc")?.ok_or_else(|| fail("no Fc"))?,
            gain_db: value("Gain")?.unwrap_or(0.0),
            q: value("Q")?.unwrap_or(std::f64::consts::FRAC_1_SQRT_2),
        });
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

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
                EqFilter {
                    kind: EqFilterKind::Peaking,
                    freq: 20.0,
                    gain_db: -1.3,
                    q: 2.0
                },
                EqFilter {
                    kind: EqFilterKind::LowShelf,
                    freq: 105.0,
                    gain_db: 5.5,
                    q: 0.7
                },
                EqFilter {
                    kind: EqFilterKind::HighShelf,
                    freq: 10000.0,
                    gain_db: 2.5,
                    q: std::f64::consts::FRAC_1_SQRT_2
                },
            ]
        );
    }

    #[test]
    fn an_unknown_filter_type_is_refused() {
        let err = parse("Filter 1: ON BP Fc 100 Hz Q 1").unwrap_err();
        assert!(err.contains("line 1"), "{err}");
    }
}
