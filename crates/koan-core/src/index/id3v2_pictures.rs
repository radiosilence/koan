//! Reading an MP3's tags without paying for the pictures in it.
//!
//! `ParseOptions::read_cover_art(false)` stops lofty *decoding* an APIC frame,
//! not reading it: `id3::v2::frame::read::skip_frame` streams the frame into
//! `io::sink()`, because at that point it holds nothing but a `Read`. Embedded
//! art is 95% of the average ID3v2 tag, so a scan pulls it all off the disk and
//! drops it on the floor — 4.1 GiB of it over a 48,000-file library.
//!
//! So we walk the frame headers ourselves first and hand lofty a reader that
//! answers with zeros over each picture, never going to the disk for those
//! bytes. lofty is contractually discarding them, so what it finds there cannot
//! change what it parses — but only while our frame arithmetic matches its own
//! exactly. Anything the walk cannot account for (an unsynchronised v2.2/v2.3
//! tag, whose byte stuffing moves lofty's stream off the file's own offsets; an
//! extended header; a frame ID that is not a frame ID) ends the walk, and the
//! rest of the tag is read the ordinary way.

use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

/// Open `path` with its ID3v2 pictures blanked out, ready to hand to lofty.
///
/// `None` if the file cannot be opened, leaving the caller to read it the
/// ordinary way.
pub(super) fn open(path: &Path) -> Option<BufReader<PictureFreeFile>> {
    let mut file = File::open(path).ok()?;
    let holes = discardable_ranges(&mut file);
    file.seek(SeekFrom::Start(0)).ok()?;
    Some(BufReader::new(PictureFreeFile {
        file,
        pos: 0,
        file_pos: 0,
        holes,
    }))
}

/// A file with holes in it: reads inside one are answered with zeros and never
/// reach the disk.
pub(super) struct PictureFreeFile {
    file: File,
    /// Where the consumer is reading from.
    pos: u64,
    /// Where the file's own cursor is, so that reading straight through costs
    /// no more seeks than reading the file directly would.
    file_pos: u64,
    /// Sorted, disjoint `[start, end)` byte ranges, in file offsets.
    holes: Vec<(u64, u64)>,
}

impl PictureFreeFile {
    /// The end of the hole `pos` falls inside, if it falls inside one.
    fn hole_end(&self, pos: u64) -> Option<u64> {
        self.holes
            .iter()
            .find(|(start, end)| pos >= *start && pos < *end)
            .map(|(_, end)| *end)
    }

    /// The start of the first hole after `pos`, which is as far as a read from
    /// `pos` may go.
    fn next_hole(&self, pos: u64) -> u64 {
        self.holes
            .iter()
            .map(|(start, _)| *start)
            .find(|start| *start > pos)
            .unwrap_or(u64::MAX)
    }
}

impl Read for PictureFreeFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }

        if let Some(end) = self.hole_end(self.pos) {
            let n = buf.len().min((end - self.pos) as usize);
            buf[..n].fill(0);
            self.pos += n as u64;
            return Ok(n);
        }

        let limit = self.next_hole(self.pos) - self.pos;
        let n = buf.len().min(limit.min(usize::MAX as u64) as usize);

        if self.file_pos != self.pos {
            self.file.seek(SeekFrom::Start(self.pos))?;
            self.file_pos = self.pos;
        }
        let read = self.file.read(&mut buf[..n])?;
        self.pos += read as u64;
        self.file_pos += read as u64;
        Ok(read)
    }
}

impl Seek for PictureFreeFile {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let pos = match from {
            SeekFrom::Start(n) => n,
            // The only seek whose answer we don't already hold. Take the real
            // file's word for the length rather than caching one.
            SeekFrom::End(n) => {
                let pos = self.file.seek(SeekFrom::End(n))?;
                self.file_pos = pos;
                pos
            }
            SeekFrom::Current(n) => self.pos.checked_add_signed(n).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "seek before start of file")
            })?,
        };
        self.pos = pos;
        Ok(pos)
    }

    fn stream_position(&mut self) -> io::Result<u64> {
        Ok(self.pos)
    }
}

/// Frame ID for an attached picture, in ID3v2.3/v2.4 and in v2.2.
const PICTURE_IDS: [&[u8]; 2] = [b"APIC", b"PIC"];

/// The byte ranges of `file`'s ID3v2 tag that lofty reads only to throw away:
/// every picture frame's payload, plus the tag's trailing padding.
///
/// Empty when there is nothing to skip, and short when the walk meets something
/// it cannot mirror lofty with — the ranges it has by then are still exact, so
/// there is no need to abandon them too.
fn discardable_ranges(file: &mut File) -> Vec<(u64, u64)> {
    let mut ranges = Vec::new();

    let mut header = [0u8; 10];
    if file.seek(SeekFrom::Start(0)).is_err()
        || file.read_exact(&mut header).is_err()
        || &header[..3] != b"ID3"
    {
        return ranges;
    }

    let version = header[3];
    let flags = header[5];

    if !matches!(version, 2..=4) {
        return ranges;
    }
    // 0x40 is an extended header in v2.3/v2.4 and an unspecified compression
    // scheme in v2.2. Either way the frames are not where they appear to be.
    if flags & 0x40 != 0 {
        return ranges;
    }
    // v2.4 unsynchronises per frame, inside the frame's own byte count, so
    // frame boundaries stay on file offsets. Before that the stuffing applied
    // to the whole tag, which moves every offset after the first 0xFF.
    if flags & 0x80 != 0 && version < 4 {
        return ranges;
    }

    let header_len: u64 = if version == 2 { 6 } else { 10 };
    let tag_end = 10
        + u64::from(unsynch(u32::from_be_bytes([
            header[6], header[7], header[8], header[9],
        ])));

    let mut cursor = 10u64;
    let mut frame = [0u8; 10];
    loop {
        if cursor + header_len > tag_end {
            break;
        }
        let frame = &mut frame[..header_len as usize];
        if file.seek(SeekFrom::Start(cursor)).is_err() || file.read_exact(frame).is_err() {
            return ranges;
        }

        // A zero where a frame ID should be is the start of the padding, which
        // is where lofty stops reading frames too.
        if frame[0] == 0 {
            break;
        }

        let (id, size) = if version == 2 {
            (
                &frame[..3],
                u32::from_be_bytes([0, frame[3], frame[4], frame[5]]),
            )
        } else {
            let size = u32::from_be_bytes([frame[4], frame[5], frame[6], frame[7]]);
            // Only v2.4 stores frame sizes synchsafe. Some v2.3 writers put a
            // v2.2 frame ID in a v2.3 header, which lofty upgrades in place.
            let size = if version == 4 { unsynch(size) } else { size };
            let id_end = if version == 3 && frame[3] == 0 { 3 } else { 4 };
            (&frame[..id_end], size)
        };

        // Past a frame ID we can't read, we can no longer say where lofty is.
        if !id
            .iter()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        {
            return ranges;
        }

        let content = cursor + header_len;
        let Some(end) = content
            .checked_add(u64::from(size))
            .filter(|end| *end <= tag_end)
        else {
            return ranges;
        };

        if PICTURE_IDS.contains(&id) && size > 0 {
            ranges.push((content, end));
        }
        cursor = end;
    }

    // Whatever is left of the tag is padding, which lofty copies into a sink.
    if cursor < tag_end {
        ranges.push((cursor, tag_end));
    }
    ranges
}

/// Decode a synchsafe integer — seven bits to the byte.
fn unsynch(n: u32) -> u32 {
    ((n & 0x7F00_0000) >> 3) | ((n & 0x7F_0000) >> 2) | ((n & 0x7F00) >> 1) | (n & 0x7F)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::generate_mp3_with_picture;

    const TITLE: &str = "Golden Skans";
    const ARTIST: &str = "Klaxons";

    /// Recognisably not zero, so a hole in the wrong place shows up as zeros
    /// where real bytes should be.
    fn picture() -> Vec<u8> {
        (0..40_000u32).map(|i| (i % 251) as u8 + 1).collect()
    }

    fn write(dir: &Path, version: u8) -> (std::path::PathBuf, std::ops::Range<u64>) {
        let path = dir.join(format!("v2{version}.mp3"));
        let range = generate_mp3_with_picture(&path, version, TITLE, ARTIST, &picture());
        (path, range)
    }

    /// The whole file as lofty sees it through the wrapper.
    fn read_through(path: &Path) -> Vec<u8> {
        let mut out = Vec::new();
        open(path).unwrap().read_to_end(&mut out).unwrap();
        out
    }

    #[test]
    fn the_walk_finds_the_picture_and_the_padding() {
        let dir = tempfile::tempdir().unwrap();
        for version in [2, 3, 4] {
            let (path, range) = write(dir.path(), version);
            let ranges = discardable_ranges(&mut File::open(&path).unwrap());

            assert_eq!(
                ranges.first().copied(),
                Some((range.start, range.end)),
                "v2.{version}: picture payload not found"
            );
            assert_eq!(ranges.len(), 2, "v2.{version}: expected picture + padding");
            let (start, end) = ranges[1];
            assert_eq!(end - start, 64, "v2.{version}: padding");
        }
    }

    /// The point of the exercise: those bytes never come off the disk.
    #[test]
    fn the_picture_reads_back_as_zeros_and_nothing_else_moves() {
        let dir = tempfile::tempdir().unwrap();
        for version in [2, 3, 4] {
            let (path, range) = write(dir.path(), version);

            let mut expected = std::fs::read(&path).unwrap();
            for (start, end) in discardable_ranges(&mut File::open(&path).unwrap()) {
                expected[start as usize..end as usize].fill(0);
            }

            let got = read_through(&path);
            assert_eq!(got.len(), expected.len(), "v2.{version}: length");
            assert_eq!(got, expected, "v2.{version}: contents");
            assert!(
                got[range.start as usize..range.end as usize]
                    .iter()
                    .all(|b| *b == 0),
                "v2.{version}: picture should have read back as zeros"
            );
        }
    }

    /// Seeks land where they would on the real file, including from the end,
    /// which is where lofty goes looking for ID3v1 and APE tags.
    #[test]
    fn seeking_matches_the_file_underneath() {
        let dir = tempfile::tempdir().unwrap();
        let (path, range) = write(dir.path(), 3);
        let real = std::fs::read(&path).unwrap();

        let mut reader = open(&path).unwrap();
        assert_eq!(
            reader.seek(SeekFrom::End(-128)).unwrap(),
            real.len() as u64 - 128
        );
        let mut tail = [0u8; 128];
        reader.read_exact(&mut tail).unwrap();
        assert_eq!(&tail[..], &real[real.len() - 128..]);

        // Back into the middle of the picture, which still answers with zeros.
        reader.seek(SeekFrom::Start(range.start + 100)).unwrap();
        let mut buf = [0xFFu8; 16];
        reader.read_exact(&mut buf).unwrap();
        assert_eq!(buf, [0u8; 16]);

        assert_eq!(
            reader.seek(SeekFrom::Current(-16)).unwrap(),
            range.start + 100
        );
    }

    /// A tag whose bytes are not where they look like they are gets no holes at
    /// all — reading it wrong is far worse than reading it slowly.
    #[test]
    fn shapes_the_walk_cannot_mirror_are_left_alone() {
        let dir = tempfile::tempdir().unwrap();

        for (name, flags, version) in [
            ("unsynchronised-v3", 0x80, 3),
            ("extended-v4", 0x40, 4),
            ("compressed-v2", 0x40, 2),
        ] {
            let (path, _) = write(dir.path(), version);
            let mut bytes = std::fs::read(&path).unwrap();
            bytes[5] = flags;
            let path = dir.path().join(format!("{name}.mp3"));
            std::fs::write(&path, &bytes).unwrap();

            assert!(
                discardable_ranges(&mut File::open(&path).unwrap()).is_empty(),
                "{name}: should have been left alone"
            );
        }

        // Not an ID3v2 tag at all.
        let path = dir.path().join("no-tag.mp3");
        std::fs::write(&path, b"\xFF\xFB\x90\x00not a tag at all").unwrap();
        assert!(discardable_ranges(&mut File::open(&path).unwrap()).is_empty());
    }
}
