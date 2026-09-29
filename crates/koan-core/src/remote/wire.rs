//! WebSockets that sleep until there is something to do.
//!
//! A link carries two kinds of traffic: what arrives from the other end, and
//! what this end has to say because something here moved — the player, the
//! queue, a command the user gave. The socket's descriptor answers the first;
//! a [`Waker`] (a pipe) answers the second. One `poll` on both, and a quiet
//! link schedules nothing between its keep-alive pings.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

/// A pipe a thread can be woken through while it waits on a socket.
pub struct Waker {
    read: OwnedFd,
    write: OwnedFd,
}

impl Waker {
    pub fn new() -> std::io::Result<Arc<Self>> {
        let mut fds = [0; 2];
        // SAFETY: `fds` has room for the two descriptors pipe writes.
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        for fd in fds {
            // SAFETY: both are descriptors pipe just returned.
            unsafe {
                libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK);
                libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            }
        }
        // SAFETY: each descriptor is owned by exactly one OwnedFd from here.
        Ok(Arc::new(unsafe {
            Self {
                read: OwnedFd::from_raw_fd(fds[0]),
                write: OwnedFd::from_raw_fd(fds[1]),
            }
        }))
    }

    /// Wake whoever waits. A full pipe already says so, so it is not an error.
    pub fn wake(&self) {
        let b = [1u8];
        // SAFETY: writes one byte from a live buffer to a descriptor we own.
        unsafe { libc::write(self.write.as_raw_fd(), b.as_ptr().cast(), 1) };
    }

    /// The end to wait on for a wake.
    pub fn read_fd(&self) -> RawFd {
        self.read.as_raw_fd()
    }

    pub fn drain(&self) {
        let mut buf = [0u8; 64];
        // SAFETY: reads into a live buffer of the length given.
        while unsafe { libc::read(self.read.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) } > 0 {
        }
    }
}

static ON_ENGINE: Mutex<Vec<Weak<Waker>>> = Mutex::new(Vec::new());

/// Wake `waker` whenever the engine says something moved, for as long as it
/// lives. One thread serves every waker in the process.
pub fn wake_on_engine_change(waker: &Arc<Waker>) {
    let mut list = ON_ENGINE.lock();
    let first = list.is_empty();
    list.push(Arc::downgrade(waker));
    drop(list);
    if first {
        std::thread::Builder::new()
            .name("koan-wire".into())
            .spawn(|| {
                let signal = crate::signal::engine_changed();
                let mut seen = signal.generation();
                loop {
                    seen = signal.wait(seen);
                    ON_ENGINE.lock().retain(|w| match w.upgrade() {
                        Some(w) => {
                            w.wake();
                            true
                        }
                        None => false,
                    });
                }
            })
            .expect("failed to spawn the wire thread");
    }
}

/// One end of a conversation over a socket.
pub trait Session {
    /// What to send now. Asked on every wake and after every message read.
    fn outgoing(&mut self) -> Vec<String>;
    fn incoming(&mut self, text: &str);
    /// Whether to hang up. Asked on every wake.
    fn done(&self) -> bool {
        false
    }
}

/// A link that has heard nothing for this long pings, so a dead connection is
/// noticed rather than waited on forever.
pub const IDLE: Duration = Duration::from_secs(45);

/// How long a connection has to answer a probe.
const PROBE_WAIT: Duration = Duration::from_secs(2);

static PROBES: AtomicU64 = AtomicU64::new(0);
static SESSIONS: Mutex<Vec<Weak<Waker>>> = Mutex::new(Vec::new());

/// Have every connection prove it is alive now, and drop those that do not
/// answer within two seconds. For an app coming back from suspension: its
/// sockets look open, but the far end gave up on them long ago, and waiting
/// for the idle ping to find out keeps the other devices out of sight.
pub fn probe_all() {
    PROBES.fetch_add(1, Ordering::Relaxed);
    SESSIONS.lock().retain(|w| match w.upgrade() {
        Some(w) => {
            w.wake();
            true
        }
        None => false,
    });
}

/// The descriptor under a client's socket, and put it in non-blocking mode.
pub fn prepare(stream: &MaybeTlsStream<TcpStream>) -> Result<RawFd, String> {
    let tcp = match stream {
        MaybeTlsStream::Plain(s) => s,
        MaybeTlsStream::Rustls(s) => s.get_ref(),
        _ => return Err("unsupported stream".into()),
    };
    tcp.set_nonblocking(true).map_err(|e| e.to_string())?;
    let _ = tcp.set_nodelay(true);
    Ok(tcp.as_raw_fd())
}

/// Run `session` over `socket` until either end goes. `fd` is the socket's
/// descriptor, already non-blocking; `waker` interrupts the wait.
pub fn drive<S: Read + Write>(
    socket: &mut WebSocket<S>,
    fd: RawFd,
    waker: &Arc<Waker>,
    session: &mut impl Session,
) -> Result<(), String> {
    let mut heard = Instant::now();
    let mut pinged = false;
    let mut probes = PROBES.load(Ordering::Relaxed);
    let mut probed: Option<Instant> = None;
    {
        let mut sessions = SESSIONS.lock();
        sessions.retain(|w| w.strong_count() > 0);
        sessions.push(Arc::downgrade(waker));
    }
    loop {
        if session.done() {
            let _ = socket.close(None);
            let _ = socket.flush();
            return Ok(());
        }
        for text in session.outgoing() {
            match socket.write(Message::Text(text.into())) {
                Ok(()) => {}
                Err(tungstenite::Error::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        let mut read_any = false;
        loop {
            match socket.read() {
                Ok(Message::Text(text)) => {
                    (heard, pinged, read_any) = (Instant::now(), false, true);
                    session.incoming(&text);
                }
                Ok(Message::Close(_)) => return Err("closed by the other end".into()),
                Ok(_) => (heard, pinged) = (Instant::now(), false),
                Err(tungstenite::Error::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    break;
                }
                Err(e) => return Err(e.to_string()),
            }
        }
        // A message read may have something to answer at once.
        if read_any {
            continue;
        }
        let blocked = match socket.flush() {
            Ok(()) => false,
            Err(tungstenite::Error::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => true,
            Err(e) => return Err(e.to_string()),
        };
        let now = PROBES.load(Ordering::Relaxed);
        if now != probes {
            probes = now;
            let _ = socket.write(Message::Ping(Vec::new().into()));
            let _ = socket.flush();
            probed = Some(Instant::now());
            continue;
        }
        let mut left = IDLE.saturating_sub(heard.elapsed());
        if let Some(at) = probed {
            if heard >= at {
                probed = None;
            } else {
                let wait = PROBE_WAIT.saturating_sub(at.elapsed());
                if wait.is_zero() {
                    return Err("no answer after waking".into());
                }
                left = left.min(wait);
            }
        }
        if left.is_zero() {
            if pinged {
                return Err("no answer to a ping".into());
            }
            let _ = socket.write(Message::Ping(Vec::new().into()));
            (heard, pinged) = (Instant::now(), true);
            continue;
        }
        let mut fds = [
            libc::pollfd {
                fd,
                events: libc::POLLIN | if blocked { libc::POLLOUT } else { 0 },
                revents: 0,
            },
            libc::pollfd {
                fd: waker.read.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let ms = left.as_millis().min(i32::MAX as u128) as i32;
        // SAFETY: `fds` is a live array of the length given.
        unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, ms) };
        waker.drain();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_waker_can_be_woken_more_than_it_is_read() {
        let waker = Waker::new().unwrap();
        for _ in 0..100_000 {
            waker.wake();
        }
        waker.drain();
        let mut fds = [libc::pollfd {
            fd: waker.read.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        }];
        // SAFETY: as above.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), 1, 0) };
        assert_eq!(ready, 0, "drained");
    }
}
