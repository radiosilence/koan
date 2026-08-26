//! `PartialFileSource` — a Read+Seek adapter over a file that is still downloading.
//!
//! The download thread writes the track to a `.part` file and publishes how far
//! it has got in an `AtomicU64`; the Symphonia decoder reads the same file
//! through a `PartialFileSource`, which blocks when the read position catches
//! up to the write head. Playback starts long before the transfer finishes and
//! seeking anywhere below the write head costs a `lseek`.
//!
//! Nothing is copied. An earlier design pumped the file into a shared `Vec<u8>`
//! so the decoder could read from memory, which cost as much RAM as the track
//! was long — half a gigabyte for a nine-hour recording, held for as long as it
//! played. The bytes are already on disk; the page cache is better at this.
//!
//! The open descriptor survives the download's final rename from `.part` to its
//! cache path, so a transfer landing mid-playback changes nothing for a reader.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Longest a read may block waiting for bytes before the transfer counts as
/// dead. A download that stops advancing must surface as an error, not park the
/// decode thread forever holding the ring buffer producer.
const STALL_LIMIT: Duration = Duration::from_secs(30);

/// How long to wait between checks for bytes that have not landed yet.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Where a download has got to, as the source needs to know it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamStatus {
    /// Bytes are still arriving.
    Downloading,
    /// Every byte landed. Reads past the end are a clean EOF.
    Complete,
    /// The transfer died before delivering everything. Reads past the written
    /// bytes fail rather than reporting EOF, which would silently truncate the
    /// track and look like a short file.
    Failed,
}

/// A `Read + Seek` view of a file that is still being written.
pub struct PartialFileSource {
    file: File,
    pos: u64,
    /// How many bytes the download has committed to disk so far.
    bytes_written: Arc<AtomicU64>,
    /// Total expected length, or 0 when the server sent no Content-Length.
    total: u64,
    status: Arc<dyn Fn() -> StreamStatus + Send + Sync>,
    stall_limit: Duration,
}

impl PartialFileSource {
    /// Open `path` for streaming. `bytes_written` is the download's own counter
    /// and `total` its advertised length, 0 when it sent none.
    pub fn open(
        path: &Path,
        bytes_written: Arc<AtomicU64>,
        total: u64,
        status: Arc<dyn Fn() -> StreamStatus + Send + Sync>,
    ) -> io::Result<Self> {
        Self::with_stall_limit(path, bytes_written, total, status, STALL_LIMIT)
    }

    fn with_stall_limit(
        path: &Path,
        bytes_written: Arc<AtomicU64>,
        total: u64,
        status: Arc<dyn Fn() -> StreamStatus + Send + Sync>,
        stall_limit: Duration,
    ) -> io::Result<Self> {
        Ok(Self {
            file: File::open(path)?,
            pos: 0,
            bytes_written,
            total,
            status,
            stall_limit,
        })
    }

    /// Bytes known to be readable — what the download has written, or the whole
    /// file once it has landed.
    fn available(&self) -> u64 {
        let written = self.bytes_written.load(Ordering::Acquire);
        match (self.status)() {
            StreamStatus::Complete => self.file.metadata().map(|m| m.len()).unwrap_or(written),
            _ => written,
        }
    }

    /// Read straight from the file, tolerating a short read at the write head:
    /// `bytes_written` is published by the downloader as it goes and the data
    /// behind it can lag by a moment.
    fn read_available(&mut self, buf: &mut [u8], limit: u64) -> io::Result<usize> {
        let to_read = (limit as usize).min(buf.len());
        self.file.read(&mut buf[..to_read]).inspect(|n| {
            self.pos += *n as u64;
        })
    }
}

impl Read for PartialFileSource {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }

        let deadline = Instant::now() + self.stall_limit;
        loop {
            let available = self.available();
            if available > self.pos {
                let n = self.read_available(buf, available - self.pos)?;
                if n > 0 {
                    return Ok(n);
                }
                // The counter ran ahead of what is visible on disk. Fall
                // through and wait rather than reporting a false EOF.
            }

            match (self.status)() {
                StreamStatus::Failed => {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "stream download failed before delivering the whole track",
                    ));
                }
                // Everything landed and there is nothing past `pos`: real EOF.
                StreamStatus::Complete if available <= self.pos => return Ok(0),
                StreamStatus::Complete => {}
                StreamStatus::Downloading => {
                    // A server that sent a Content-Length has delivered it all.
                    if self.total > 0 && available >= self.total && self.pos >= self.total {
                        return Ok(0);
                    }
                }
            }

            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "stream download stalled",
                ));
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }
}

impl Seek for PartialFileSource {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let target: i64 = match pos {
            SeekFrom::Start(n) => n as i64,
            SeekFrom::Current(n) => self.pos as i64 + n,
            // Seeking relative to an end the download has not reached yet is
            // guesswork; the advertised length is the best answer there is.
            SeekFrom::End(n) => {
                let len = if self.total > 0 {
                    self.total
                } else {
                    self.available()
                };
                len as i64 + n
            }
        };

        if target < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek before beginning of stream",
            ));
        }

        self.pos = self.file.seek(SeekFrom::Start(target as u64))?;
        Ok(self.pos)
    }
}

// Symphonia requires MediaSource: Read + Seek + Send + Any
impl symphonia::core::io::MediaSource for PartialFileSource {
    fn is_seekable(&self) -> bool {
        // Backward seeks and forward seeks below the write head are a `lseek`
        // on a file that is already there. A forward seek past it lands on a
        // read that blocks until the bytes arrive, which is the honest
        // behaviour — callers clamp to `seekable_ms` to avoid asking.
        true
    }

    fn byte_len(&self) -> Option<u64> {
        (self.total > 0).then_some(self.total)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::atomic::AtomicU8;

    use symphonia::core::io::MediaSource;

    use super::*;

    /// A file plus the counter and status a download would publish, so a test
    /// can advance either independently.
    struct Fixture {
        _dir: tempfile::TempDir,
        path: std::path::PathBuf,
        written: Arc<AtomicU64>,
        status: Arc<AtomicU8>,
    }

    const DOWNLOADING: u8 = 0;
    const COMPLETE: u8 = 1;
    const FAILED: u8 = 2;

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("track.opus.part");
            File::create(&path).unwrap();
            Self {
                _dir: dir,
                path,
                written: Arc::new(AtomicU64::new(0)),
                status: Arc::new(AtomicU8::new(DOWNLOADING)),
            }
        }

        /// Append bytes and publish them, as the downloader does per chunk.
        fn push(&self, chunk: &[u8]) {
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&self.path)
                .unwrap();
            f.write_all(chunk).unwrap();
            f.flush().unwrap();
            self.written
                .fetch_add(chunk.len() as u64, Ordering::Release);
        }

        fn set(&self, status: u8) {
            self.status.store(status, Ordering::Release);
        }

        fn source(&self, total: u64) -> PartialFileSource {
            self.source_with_stall(total, STALL_LIMIT)
        }

        fn source_with_stall(&self, total: u64, stall: Duration) -> PartialFileSource {
            let status = self.status.clone();
            PartialFileSource::with_stall_limit(
                &self.path,
                self.written.clone(),
                total,
                Arc::new(move || match status.load(Ordering::Acquire) {
                    COMPLETE => StreamStatus::Complete,
                    FAILED => StreamStatus::Failed,
                    _ => StreamStatus::Downloading,
                }),
                stall,
            )
            .unwrap()
        }
    }

    #[test]
    fn reads_what_has_landed() {
        let fx = Fixture::new();
        fx.push(b"hello streaming world");
        fx.set(COMPLETE);

        let mut out = Vec::new();
        fx.source(21).read_to_end(&mut out).unwrap();
        assert_eq!(out, b"hello streaming world");
    }

    #[test]
    fn read_stops_at_the_write_head_then_resumes() {
        let fx = Fixture::new();
        fx.push(b"abcd");
        let mut src = fx.source(10);

        let mut first = [0u8; 8];
        assert_eq!(src.read(&mut first).unwrap(), 4);
        assert_eq!(&first[..4], b"abcd");

        // The rest arrives while the reader is blocked on it.
        std::thread::spawn({
            let path = fx.path.clone();
            let written = fx.written.clone();
            move || {
                std::thread::sleep(Duration::from_millis(20));
                let mut f = std::fs::OpenOptions::new()
                    .append(true)
                    .open(&path)
                    .unwrap();
                f.write_all(b"efghij").unwrap();
                f.flush().unwrap();
                written.fetch_add(6, Ordering::Release);
            }
        });

        let mut rest = [0u8; 8];
        let n = src.read(&mut rest).unwrap();
        assert_eq!(&rest[..n], b"efghij");
    }

    #[test]
    fn seeks_freely_below_the_write_head() {
        let fx = Fixture::new();
        fx.push(b"0123456789");
        let mut src = fx.source(1_000_000);

        assert_eq!(src.seek(SeekFrom::Start(5)).unwrap(), 5);
        let mut out = [0u8; 3];
        src.read_exact(&mut out).unwrap();
        assert_eq!(&out, b"567");

        // Backwards, into bytes already read — no re-download, no buffer.
        assert_eq!(src.seek(SeekFrom::Start(1)).unwrap(), 1);
        src.read_exact(&mut out).unwrap();
        assert_eq!(&out, b"123");

        assert_eq!(src.seek(SeekFrom::Current(-2)).unwrap(), 2);
    }

    #[test]
    fn seek_from_end_uses_the_advertised_length() {
        let fx = Fixture::new();
        fx.push(b"0123456789");
        let mut src = fx.source(10);

        assert_eq!(src.seek(SeekFrom::End(0)).unwrap(), 10);
        assert_eq!(src.seek(SeekFrom::End(-3)).unwrap(), 7);

        let mut out = [0u8; 3];
        src.read_exact(&mut out).unwrap();
        assert_eq!(&out, b"789");
    }

    #[test]
    fn seek_before_start_errors() {
        let fx = Fixture::new();
        fx.push(b"hello");
        assert!(fx.source(5).seek(SeekFrom::Current(-1)).is_err());
    }

    #[test]
    fn failed_download_errors_instead_of_reporting_eof() {
        let fx = Fixture::new();
        fx.push(b"partial");
        fx.set(FAILED);
        let mut src = fx.source(1000);

        let mut out = [0u8; 7];
        src.read_exact(&mut out).unwrap();
        assert_eq!(&out, b"partial");

        // Past the written bytes: an error, never a clean EOF — Ok(0) here
        // would end the track early and look like a short file.
        assert_eq!(
            src.read(&mut out).unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
    }

    #[test]
    fn failure_wakes_a_blocked_reader() {
        let fx = Fixture::new();
        let mut src = fx.source(1000);

        let status = fx.status.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            status.store(FAILED, Ordering::Release);
        });

        let mut out = [0u8; 8];
        assert_eq!(
            src.read(&mut out).unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
    }

    #[test]
    fn stalled_download_times_out() {
        // A download with a Content-Length that never arrives: the read must
        // give up rather than park the decode thread forever.
        let fx = Fixture::new();
        let mut src = fx.source_with_stall(1000, Duration::from_millis(20));
        let mut out = [0u8; 8];
        assert_eq!(
            src.read(&mut out).unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
    }

    #[test]
    fn completion_ends_the_read_at_the_true_length() {
        // A chunked transfer reports no total; completion is what says the file
        // is whole, and its length on disk is what is readable.
        let fx = Fixture::new();
        fx.push(b"chunked");
        fx.set(COMPLETE);

        let mut out = Vec::new();
        fx.source(0).read_to_end(&mut out).unwrap();
        assert_eq!(out, b"chunked");
    }

    #[test]
    fn survives_the_part_file_being_renamed() {
        // The download's final act is a rename. A reader that already has the
        // file open must not notice.
        let fx = Fixture::new();
        fx.push(b"0123456789");
        let mut src = fx.source(10);

        let mut out = [0u8; 4];
        src.read_exact(&mut out).unwrap();
        assert_eq!(&out, b"0123");

        std::fs::rename(&fx.path, fx.path.with_extension("")).unwrap();
        fx.set(COMPLETE);

        let mut rest = Vec::new();
        src.read_to_end(&mut rest).unwrap();
        assert_eq!(rest, b"456789");
    }

    #[test]
    fn byte_len_is_the_advertised_length_only() {
        let fx = Fixture::new();
        assert_eq!(fx.source(42).byte_len(), Some(42));
        // No Content-Length: the length is genuinely unknown, and claiming one
        // would have Symphonia compute a duration from it.
        assert_eq!(fx.source(0).byte_len(), None);
    }

    #[test]
    fn is_seekable_true() {
        let fx = Fixture::new();
        assert!(fx.source(0).is_seekable());
    }
}
