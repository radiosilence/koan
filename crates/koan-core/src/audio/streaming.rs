//! StreamingSource — a Read+Seek adapter over a shared, incrementally-filled byte buffer.
//!
//! The download thread writes chunks into a `StreamBuffer` while the Symphonia decoder
//! reads from a `StreamingSource` backed by the same buffer. The source blocks briefly
//! when the read position catches up to the write head, enabling true streaming decode
//! without waiting for the full download to complete.

use std::io::{self, Read, Seek, SeekFrom};
use std::sync::{Arc, Condvar, Mutex};

/// State shared between the download writer and the decoder reader.
struct Inner {
    data: Vec<u8>,
    /// Total expected byte length. `None` if not yet known (no Content-Length).
    total_len: Option<u64>,
    /// Set to true when the download thread has finished (EOF or error).
    done: bool,
}

/// A shared, growable byte buffer that the download thread writes into.
///
/// Clone it to get additional handles; all clones share the same underlying data.
#[derive(Clone)]
pub struct StreamBuffer {
    inner: Arc<(Mutex<Inner>, Condvar)>,
}

impl StreamBuffer {
    /// Create a new empty buffer. `total_len` may be provided once Content-Length is known.
    pub fn new(total_len: Option<u64>) -> Self {
        Self {
            inner: Arc::new((
                Mutex::new(Inner {
                    data: Vec::new(),
                    total_len,
                    done: false,
                }),
                Condvar::new(),
            )),
        }
    }

    /// Append downloaded bytes. Called by the download thread.
    pub fn push(&self, chunk: &[u8]) {
        let (lock, cvar) = &*self.inner;
        let mut inner = lock.lock().unwrap();
        inner.data.extend_from_slice(chunk);
        cvar.notify_all();
    }

    /// Signal that the download is complete (or failed). No more bytes will arrive.
    pub fn finish(&self) {
        let (lock, cvar) = &*self.inner;
        let mut inner = lock.lock().unwrap();
        inner.done = true;
        cvar.notify_all();
    }

    /// Total bytes received so far.
    pub fn bytes_downloaded(&self) -> u64 {
        let (lock, _) = &*self.inner;
        lock.lock().unwrap().data.len() as u64
    }

    /// Total expected length (from Content-Length), if known.
    pub fn total_len(&self) -> Option<u64> {
        let (lock, _) = &*self.inner;
        lock.lock().unwrap().total_len
    }

    /// Create a `StreamingSource` that reads from this buffer starting at offset 0.
    pub fn reader(&self) -> StreamingSource {
        StreamingSource {
            inner: self.inner.clone(),
            pos: 0,
        }
    }
}

/// A `Read + Seek` view into a `StreamBuffer`.
///
/// Blocks on `read` when the read position is at or beyond the write head,
/// until more bytes arrive or the download finishes.
pub struct StreamingSource {
    inner: Arc<(Mutex<Inner>, Condvar)>,
    pos: u64,
}

impl Read for StreamingSource {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }

        let (lock, cvar) = &*self.inner;

        // Wait until there is data at `pos`, or the download is done.
        let inner = cvar
            .wait_while(lock.lock().unwrap(), |s| {
                s.data.len() as u64 <= self.pos && !s.done
            })
            .unwrap();

        let available = inner.data.len() as u64;
        if available <= self.pos {
            // Done and no more data — EOF.
            return Ok(0);
        }

        let start = self.pos as usize;
        let end = (start + buf.len()).min(inner.data.len());
        let n = end - start;
        buf[..n].copy_from_slice(&inner.data[start..end]);
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for StreamingSource {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let (lock, _) = &*self.inner;
        let inner = lock.lock().unwrap();

        let new_pos: i64 = match pos {
            SeekFrom::Start(n) => n as i64,
            SeekFrom::Current(n) => self.pos as i64 + n,
            SeekFrom::End(n) => {
                // For End seeks we need total_len. If not known yet, use current data len.
                let len = inner.total_len.unwrap_or(inner.data.len() as u64) as i64;
                len + n
            }
        };

        if new_pos < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek before beginning of stream",
            ));
        }

        self.pos = new_pos as u64;
        Ok(self.pos)
    }
}

// Symphonia requires MediaSource: Read + Seek + Send + Any
impl symphonia::core::io::MediaSource for StreamingSource {
    fn is_seekable(&self) -> bool {
        // Seekable only if the total length is known (needed for seek-to-end math).
        // Forward seeks always work; backward seeks require buffered data already present.
        // We advertise seekable=true and handle backward seeks via the buffered Vec.
        true
    }

    fn byte_len(&self) -> Option<u64> {
        let (lock, _) = &*self.inner;
        lock.lock().unwrap().total_len
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Seek, SeekFrom};

    use super::*;

    fn filled_buffer(data: &[u8]) -> StreamBuffer {
        let buf = StreamBuffer::new(Some(data.len() as u64));
        buf.push(data);
        buf.finish();
        buf
    }

    #[test]
    fn new_buffer_starts_empty() {
        let buf = StreamBuffer::new(Some(1024));
        assert_eq!(buf.bytes_downloaded(), 0);
        assert_eq!(buf.total_len(), Some(1024));
    }

    #[test]
    fn new_buffer_unknown_total() {
        let buf = StreamBuffer::new(None);
        assert_eq!(buf.total_len(), None);
    }

    #[test]
    fn read_all_data_available() {
        let data = b"hello streaming world";
        let buf = filled_buffer(data);
        let mut src = buf.reader();

        let mut out = Vec::new();
        src.read_to_end(&mut out).unwrap();
        assert_eq!(out, data);
    }

    #[test]
    fn read_partial_then_rest() {
        let data = b"abcdefghij";
        let buf = filled_buffer(data);
        let mut src = buf.reader();

        let mut first = [0u8; 4];
        let n = src.read(&mut first).unwrap();
        assert_eq!(n, 4);
        assert_eq!(&first, b"abcd");

        let mut rest = Vec::new();
        src.read_to_end(&mut rest).unwrap();
        assert_eq!(rest, b"efghij");
    }

    #[test]
    fn seek_from_start() {
        let data = b"0123456789";
        let buf = filled_buffer(data);
        let mut src = buf.reader();

        let pos = src.seek(SeekFrom::Start(5)).unwrap();
        assert_eq!(pos, 5);

        let mut out = [0u8; 3];
        src.read_exact(&mut out).unwrap();
        assert_eq!(&out, b"567");
    }

    #[test]
    fn seek_from_current() {
        let data = b"0123456789";
        let buf = filled_buffer(data);
        let mut src = buf.reader();

        src.seek(SeekFrom::Start(2)).unwrap();
        let pos = src.seek(SeekFrom::Current(3)).unwrap();
        assert_eq!(pos, 5);

        let mut out = [0u8; 2];
        src.read_exact(&mut out).unwrap();
        assert_eq!(&out, b"56");
    }

    #[test]
    fn seek_from_end() {
        let data = b"0123456789";
        let buf = filled_buffer(data);
        let mut src = buf.reader();

        // SeekFrom::End(0) should position at total_len (EOF).
        let pos = src.seek(SeekFrom::End(0)).unwrap();
        assert_eq!(pos, 10);

        // SeekFrom::End(-3) should position at offset 7.
        let pos = src.seek(SeekFrom::End(-3)).unwrap();
        assert_eq!(pos, 7);

        let mut out = [0u8; 3];
        src.read_exact(&mut out).unwrap();
        assert_eq!(&out, b"789");
    }

    #[test]
    fn seek_before_start_errors() {
        let data = b"hello";
        let buf = filled_buffer(data);
        let mut src = buf.reader();

        let result = src.seek(SeekFrom::Current(-1));
        assert!(result.is_err());
    }

    #[test]
    fn is_complete_when_done() {
        let buf = StreamBuffer::new(Some(5));
        buf.push(b"hello");
        // Not yet finished.
        assert!(buf.bytes_downloaded() != 0); // just check bytes_downloaded works
        buf.finish();
        // After finish, a reader should see EOF immediately.
        let mut src = buf.reader();
        let mut out = Vec::new();
        src.read_to_end(&mut out).unwrap();
        assert_eq!(out, b"hello");
    }

    #[test]
    fn byte_len_returns_total() {
        use symphonia::core::io::MediaSource;
        let buf = StreamBuffer::new(Some(42));
        let src = buf.reader();
        assert_eq!(src.byte_len(), Some(42));
    }

    #[test]
    fn is_seekable_true() {
        use symphonia::core::io::MediaSource;
        let buf = StreamBuffer::new(None);
        let src = buf.reader();
        assert!(src.is_seekable());
    }

    #[test]
    fn push_increments_bytes_downloaded() {
        let buf = StreamBuffer::new(Some(10));
        buf.push(b"hello");
        assert_eq!(buf.bytes_downloaded(), 5);
        buf.push(b"world");
        assert_eq!(buf.bytes_downloaded(), 10);
    }

    #[test]
    fn multiple_readers_independent_positions() {
        let data = b"0123456789";
        let buf = filled_buffer(data);

        let mut r1 = buf.reader();
        let mut r2 = buf.reader();

        r1.seek(SeekFrom::Start(7)).unwrap();

        let mut out1 = [0u8; 3];
        r1.read_exact(&mut out1).unwrap();
        assert_eq!(&out1, b"789");

        let mut out2 = [0u8; 3];
        r2.read_exact(&mut out2).unwrap();
        assert_eq!(&out2, b"012");
    }

    // --- Tests using the requested names ---

    #[test]
    fn test_new_and_is_complete() {
        // "is_complete" == bytes_downloaded() == total_len and done==true (finish() called).
        let data = vec![0u8; 1000];
        let buf = StreamBuffer::new(Some(1000));
        buf.push(&data);
        buf.finish();
        assert_eq!(buf.bytes_downloaded(), 1000);
        assert_eq!(buf.total_len(), Some(1000));
        // A reader should see EOF immediately (no blocking).
        let mut src = buf.reader();
        let mut out = Vec::new();
        src.read_to_end(&mut out).unwrap();
        assert_eq!(out.len(), 1000);
    }

    #[test]
    fn test_read_all_available() {
        let data: Vec<u8> = (0u8..=255).cycle().take(1000).collect();
        let buf = StreamBuffer::new(Some(1000));
        buf.push(&data);
        buf.finish();
        let mut src = buf.reader();
        let mut out = Vec::new();
        src.read_to_end(&mut out).unwrap();
        assert_eq!(out, data);
    }

    #[test]
    fn test_seek_start() {
        let data: Vec<u8> = (0u8..=255).cycle().take(1000).collect();
        let buf = filled_buffer(&data);
        let mut src = buf.reader();

        src.seek(SeekFrom::Start(500)).unwrap();
        let mut out = Vec::new();
        src.read_to_end(&mut out).unwrap();
        assert_eq!(out, &data[500..]);
    }

    #[test]
    fn test_seek_current() {
        let data: Vec<u8> = (0u8..=255).cycle().take(1000).collect();
        let buf = filled_buffer(&data);
        let mut src = buf.reader();

        // Read 100 bytes then seek forward 400 from current → position 500.
        let mut tmp = vec![0u8; 100];
        src.read_exact(&mut tmp).unwrap();
        let pos = src.seek(SeekFrom::Current(400)).unwrap();
        assert_eq!(pos, 500);

        let mut out = Vec::new();
        src.read_to_end(&mut out).unwrap();
        assert_eq!(out, &data[500..]);
    }

    #[test]
    fn test_partial_availability() {
        // Push only 500 bytes without finish() — simulates in-progress download.
        let data: Vec<u8> = (0u8..=255).cycle().take(1000).collect();
        let buf = StreamBuffer::new(Some(1000));
        buf.push(&data[..500]);

        assert_eq!(buf.bytes_downloaded(), 500);
        assert_eq!(buf.total_len(), Some(1000));

        // Spawn thread to call finish() after brief delay so reader doesn't block forever.
        let buf2 = buf.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(10));
            buf2.finish();
        });

        let mut src = buf.reader();
        let mut out = Vec::new();
        src.read_to_end(&mut out).unwrap();
        assert_eq!(out.len(), 500);
        assert_eq!(out, &data[..500]);
    }
}
