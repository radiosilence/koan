//! Impulse responses without a container: headerless sample files, as
//! CamillaDSP and BruteFIR read them, and coefficients written one per line, as
//! REW and CamillaDSP's `TEXT` format write them. Neither says what rate it is
//! at; that comes from whatever refers to them, or from whoever imports them.

use std::io::Write;
use std::path::Path;

/// How a headerless file stores its samples, in CamillaDSP's names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawFormat {
    S16Le,
    S24Le3,
    S24Le,
    S32Le,
    F32Le,
    F64Le,
    Text,
}

impl RawFormat {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name.to_ascii_uppercase().as_str() {
            "S16LE" => Self::S16Le,
            "S24LE3" => Self::S24Le3,
            "S24LE" => Self::S24Le,
            "S32LE" => Self::S32Le,
            "FLOAT32LE" => Self::F32Le,
            "FLOAT64LE" => Self::F64Le,
            "TEXT" => Self::Text,
            _ => return None,
        })
    }
}

/// Read `path` as `format`. `skip` and `read` are in bytes for binary formats
/// and lines for text, as CamillaDSP's `skip_bytes_lines` and
/// `read_bytes_lines` are; a `read` of 0 reads to the end.
pub fn read(path: &Path, format: RawFormat, skip: usize, read: usize) -> Result<Vec<f32>, String> {
    if format == RawFormat::Text {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let lines = text.lines().skip(skip);
        let lines: Vec<&str> = if read > 0 {
            lines.take(read).collect()
        } else {
            lines.collect()
        };
        return parse_text(&lines.join("\n"));
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let bytes = bytes.get(skip..).unwrap_or_default();
    let bytes = if read > 0 {
        &bytes[..read.min(bytes.len())]
    } else {
        bytes
    };
    let samples: Vec<f32> = match format {
        RawFormat::S16Le => bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| i16::from_le_bytes(*b) as f32 / 32768.0)
            .collect(),
        RawFormat::S24Le3 => bytes
            .as_chunks::<3>()
            .0
            .iter()
            .map(|b| (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0)
            .collect(),
        RawFormat::S24Le => bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0)
            .collect(),
        RawFormat::S32Le => bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| i32::from_le_bytes(*b) as f32 / 2_147_483_648.0)
            .collect(),
        RawFormat::F32Le => bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect(),
        RawFormat::F64Le => bytes
            .as_chunks::<8>()
            .0
            .iter()
            .map(|b| f64::from_le_bytes(*b) as f32)
            .collect(),
        RawFormat::Text => unreachable!("handled above"),
    };
    if samples.is_empty() {
        return Err("no samples".into());
    }
    Ok(samples)
}

/// Coefficients one per line. Header lines that are not numbers — REW writes
/// a few, starting `*` — are skipped where they lead; a stray one further down
/// means this is not a list of coefficients.
pub fn parse_text(text: &str) -> Result<Vec<f32>, String> {
    let mut samples = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // REW writes "time value" pairs in some exports; the value is last.
        let value = line.split([' ', '\t', ',', ';']).rfind(|s| !s.is_empty());
        match value.and_then(|v| v.parse::<f32>().ok()) {
            Some(v) => samples.push(v),
            None if samples.is_empty() => continue,
            None => return Err(format!("not a coefficient: {line}")),
        }
    }
    if samples.is_empty() {
        return Err("no coefficients".into());
    }
    Ok(samples)
}

/// Whether `text` reads as coefficients rather than a configuration.
pub fn looks_like_coefficients(text: &str) -> bool {
    let mut numbers = 0;
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if line
            .split([' ', '\t', ',', ';'])
            .rfind(|s| !s.is_empty())
            .is_some_and(|v| v.parse::<f32>().is_ok())
        {
            numbers += 1;
        } else if numbers > 0 {
            return false;
        }
    }
    numbers >= 16
}

/// Write `channels` as a 32-bit float WAV at `rate`: how koan keeps every
/// impulse response, whatever it was imported from.
pub fn write_wav(path: &Path, rate: u32, channels: &[Vec<f32>]) -> std::io::Result<()> {
    let n = channels.len() as u16;
    let frames = channels.iter().map(Vec::len).max().unwrap_or(0);
    let data_len = (frames * n as usize * 4) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&3u16.to_le_bytes()); // IEEE float
    out.extend_from_slice(&n.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * n as u32 * 4).to_le_bytes());
    out.extend_from_slice(&(n * 4).to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..frames {
        for ch in channels {
            out.extend_from_slice(&ch.get(i).copied().unwrap_or(0.0).to_le_bytes());
        }
    }
    std::fs::File::create(path)?.write_all(&out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rew_text_with_a_header_reads() {
        let text = "* Impulse Response data saved by REW\n* Source: x\n0.0\n0.5\n-0.25\n";
        assert_eq!(parse_text(text).unwrap(), vec![0.0, 0.5, -0.25]);
        assert!(parse_text("1.0\nFilter 1: ON PK\n").is_err());
    }

    #[test]
    fn binary_formats_read() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("ir.raw");
        std::fs::write(&p, [0x00u8, 0x40, 0x00, 0xc0]).unwrap();
        assert_eq!(read(&p, RawFormat::S16Le, 0, 0).unwrap(), vec![0.5, -0.5]);
        std::fs::write(&p, 0.25f32.to_le_bytes()).unwrap();
        assert_eq!(read(&p, RawFormat::F32Le, 0, 0).unwrap(), vec![0.25]);
        std::fs::write(&p, [0u8, 0, 0x40, 0, 0, 0xc0]).unwrap();
        assert_eq!(read(&p, RawFormat::S24Le3, 0, 0).unwrap(), vec![0.5, -0.5]);
    }

    #[test]
    fn a_written_wav_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("ir.wav");
        write_wav(&p, 96000, &[vec![1.0, 0.5], vec![0.25, -0.125]]).unwrap();
        let (rate, channels) = super::super::impulse::read_audio(&p).unwrap();
        assert_eq!(rate, 96000);
        assert_eq!(channels, vec![vec![1.0, 0.5], vec![0.25, -0.125]]);
    }
}
