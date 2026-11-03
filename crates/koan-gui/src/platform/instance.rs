//! One desktop app per configuration directory. A second launch hands its
//! files to the first and exits, and the first comes to the front.
//!
//! On macOS a bundled app is kept to one instance by LaunchServices, which
//! delivers files and links through `on_open_urls`; this covers the bare
//! binary there, and Linux, where nothing else does.
//!
//! A lock in the configuration directory, beside the device's own, says who
//! serves: whoever holds it listens on a Unix socket and writes the socket's
//! path into the lock file, and anyone else reads the path there, connects,
//! and writes the absolute paths it was given, one per line. The socket lives
//! in a directory short and private enough for one, which can differ between
//! two launches (a desktop session and an ssh one); the lock is where both
//! find it. The system lets go of the lock however the holder ends, so a
//! socket left behind by a crash is replaced by the next holder.

use std::fs::{File, OpenOptions};
use std::io::{BufRead as _, BufReader, ErrorKind, Read as _, Write as _};
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use futures_channel::mpsc::UnboundedSender;

/// What reaches the running app from outside it.
#[derive(Debug, PartialEq)]
pub enum Opened {
    /// Files and folders to import and queue.
    Paths(Vec<PathBuf>),
    /// Links that are not files, such as invites and pairings, for the shell.
    Links(Vec<String>),
}

/// How long a second launch keeps trying to reach the first, which may still
/// be starting.
const CONNECT_FOR: Duration = Duration::from_secs(3);

/// How long the first waits for a second launch to finish writing.
const READ_FOR: Duration = Duration::from_secs(2);

/// The most a second launch may send: a selection of several thousand files.
const MOST: u64 = 1 << 20;

pub enum Launch {
    /// This process serves. Holding the value holds the lock.
    First(Server),
    /// Another process serves and has been handed the paths.
    Handed,
}

pub struct Server {
    _lock: File,
    listener: UnixListener,
}

/// The directory the socket lives in, private to this user and short enough
/// for a socket's path (104 bytes on macOS, 108 on Linux): the session's
/// runtime directory on Linux, macOS's per-user temporary directory, and the
/// configuration directory on a Linux session without one.
fn runtime_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        return std::env::temp_dir();
    }
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|d| d.is_absolute())
        .unwrap_or_else(koan_core::config::config_dir)
}

/// A name for `config_dir` that is the same in every build, so two launches
/// against one configuration meet and launches against two do not.
/// FNV-1a: std's hasher is free to change between Rust releases.
fn name_of(config_dir: &Path) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in config_dir.as_os_str().as_encoded_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("koan-gui-{hash:016x}")
}

/// Become the instance for this configuration, or hand `paths` to the one
/// that is. `Err` when neither is possible: the lock or socket cannot be
/// made, or the instance holding the lock does not answer.
pub fn claim(paths: &[PathBuf]) -> std::io::Result<Launch> {
    let config = koan_core::config::config_dir();
    // An earlier version locks inside the config directory. While one runs
    // it is the instance: its socket has the same name, so the hand-off
    // meets it.
    if let Some(mut earlier) = earlier_version_lock() {
        hand(&mut earlier, paths)?;
        return Ok(Launch::Handed);
    }
    // One lock per configuration, outside it: the config directory holds
    // what a person chooses, not what the app writes as it runs.
    let locks = koan_core::places::runtime();
    std::fs::create_dir_all(&locks)?;
    let name = name_of(&config);
    let mut lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(locks.join("gui-instance.lock"))?;
    match lock.try_lock() {
        Ok(()) => {
            // An earlier holder's socket path goes first, so a launch reading
            // the lock while this one binds waits rather than follows it.
            lock.set_len(0)?;
            let dir = runtime_dir();
            std::fs::create_dir_all(&dir)?;
            let socket = dir.join(format!("{name}.sock"));
            let listener = bind_private(&socket)?;
            lock.write_all(socket.as_os_str().as_encoded_bytes())?;
            Ok(Launch::First(Server {
                _lock: lock,
                listener,
            }))
        }
        Err(std::fs::TryLockError::WouldBlock) => {
            hand(&mut lock, paths)?;
            Ok(Launch::Handed)
        }
        Err(std::fs::TryLockError::Error(e)) => Err(e),
    }
}

/// The lock an earlier version takes in the config directory, while a
/// running one holds it.
pub fn earlier_version_lock() -> Option<File> {
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(koan_core::config::config_dir().join(EARLIER_LOCK))
        .ok()?;
    match lock.try_lock() {
        Err(std::fs::TryLockError::WouldBlock) => Some(lock),
        _ => None,
    }
}

/// Where an earlier version kept the instance lock.
pub const EARLIER_LOCK: &str = "gui-instance.lock";

/// Listen at `socket`, connectable by this user alone from the moment it
/// can be found: bound under a name of its own, made 0600, then moved into
/// place over whatever an earlier holder left there.
fn bind_private(socket: &Path) -> std::io::Result<UnixListener> {
    let staged = socket.with_extension(format!("{}.new", std::process::id()));
    match std::fs::remove_file(&staged) {
        Err(e) if e.kind() != ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let listener = UnixListener::bind(&staged)?;
    let placed = std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o600))
        .and_then(|()| std::fs::rename(&staged, socket));
    if let Err(e) = placed {
        let _ = std::fs::remove_file(&staged);
        return Err(e);
    }
    Ok(listener)
}

/// Write `paths` to the instance holding `lock`, at the socket it named
/// there. The holder may not have named it yet, or be listening yet, so both
/// are tried until a deadline.
fn hand(lock: &mut File, paths: &[PathBuf]) -> std::io::Result<()> {
    let deadline = Instant::now() + CONNECT_FOR;
    let mut stream = loop {
        let attempt = socket_named(lock).and_then(|socket| UnixStream::connect(&socket));
        match attempt {
            Ok(stream) => break stream,
            Err(e) if Instant::now() < deadline => {
                log::debug!("instance: waiting for the running app: {e}");
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(e),
        }
    };
    stream.set_write_timeout(Some(READ_FOR))?;
    stream.write_all(&encode(paths))?;
    stream.shutdown(std::net::Shutdown::Write)?;
    // Held open until the running app has read it and closed its end. A
    // connection whose sender has already gone fails the holder's accept on
    // macOS (EINVAL), and the hand-off, files and coming to the front, would
    // be lost.
    stream.set_read_timeout(Some(READ_FOR))?;
    let _ = stream.read(&mut [0u8; 1]);
    Ok(())
}

/// The socket the lock's holder wrote into it.
fn socket_named(lock: &mut File) -> std::io::Result<PathBuf> {
    use std::io::Seek as _;
    let mut bytes = Vec::new();
    lock.seek(std::io::SeekFrom::Start(0))?;
    lock.read_to_end(&mut bytes)?;
    if bytes.is_empty() {
        return Err(std::io::Error::new(
            ErrorKind::NotFound,
            "the running app has not named its socket yet",
        ));
    }
    // SAFETY: written by `claim` from an OsStr on this platform, where any
    // byte sequence is a valid OsStr.
    Ok(PathBuf::from(unsafe {
        std::ffi::OsString::from_encoded_bytes_unchecked(bytes)
    }))
}

fn encode(paths: &[PathBuf]) -> Vec<u8> {
    let mut out = Vec::new();
    for path in paths {
        // A path holding a newline cannot be sent this way and is left out.
        let bytes = path.as_os_str().as_encoded_bytes();
        if !bytes.contains(&b'\n') {
            out.extend_from_slice(bytes);
            out.push(b'\n');
        }
    }
    out
}

fn decode(read: impl std::io::Read) -> Vec<PathBuf> {
    BufReader::new(read)
        .split(b'\n')
        .map_while(Result::ok)
        .filter(|line| !line.is_empty())
        .map(|line| {
            // SAFETY: the bytes were written by `encode` from an OsStr on this
            // platform, where any byte sequence is a valid OsStr.
            PathBuf::from(unsafe { std::ffi::OsString::from_encoded_bytes_unchecked(line) })
        })
        .collect()
}

impl Server {
    /// Hand every later launch's paths to `opened`, from a thread that lives
    /// as long as the process. Each connection is read on a thread of its
    /// own, to a deadline and a size, so one that never writes holds up
    /// nobody.
    pub fn serve(self, opened: UnboundedSender<Opened>) {
        let Server { _lock, listener } = self;
        std::thread::Builder::new()
            .name("koan-gui-instance".into())
            .spawn(move || {
                let _lock = _lock;
                for stream in listener.incoming() {
                    let stream = match stream {
                        Ok(stream) => stream,
                        Err(e) => {
                            log::warn!("instance: {e}");
                            continue;
                        }
                    };
                    let opened = opened.clone();
                    let read = std::thread::Builder::new()
                        .name("koan-gui-handover".into())
                        .spawn(move || {
                            if let Err(e) = stream.set_read_timeout(Some(READ_FOR)) {
                                log::warn!("instance: {e}");
                                return;
                            }
                            let paths = decode(stream.take(MOST));
                            // Empty is a launch with nothing to open: come to
                            // the front all the same.
                            let _ = opened.unbounded_send(Opened::Paths(paths));
                        });
                    if let Err(e) = read {
                        log::warn!("instance: {e}");
                    }
                }
            })
            .expect("failed to spawn the instance thread");
    }
}

/// The paths named on the command line, made absolute against the directory
/// it was run in, since the instance they are handed to runs elsewhere. An
/// argument starting with `-` is a flag, whatever a launcher meant by it, and
/// is left out.
pub fn argv_paths() -> Vec<PathBuf> {
    let cwd = std::env::current_dir().unwrap_or_default();
    std::env::args_os()
        .skip(1)
        .filter(|a| !a.as_encoded_bytes().starts_with(b"-"))
        .map(PathBuf::from)
        .map(|p| if p.is_absolute() { p } else { cwd.join(p) })
        .collect()
}

/// What the platform's open request names: `file://` URLs are paths, and
/// anything else is a link for the shell.
pub fn sort_urls(urls: Vec<String>) -> Vec<Opened> {
    let (files, links): (Vec<_>, Vec<_>) = urls.into_iter().partition(|u| u.starts_with("file://"));
    let paths: Vec<PathBuf> = files.iter().filter_map(|u| file_path(u)).collect();
    let mut out = Vec::new();
    if !paths.is_empty() {
        out.push(Opened::Paths(paths));
    }
    if !links.is_empty() {
        out.push(Opened::Links(links));
    }
    out
}

/// The path a `file://` URL names, percent-decoded.
fn file_path(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    // `file://localhost/x` and `file:///x` both name `/x`.
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    if !rest.starts_with('/') {
        return None;
    }
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(byte) = bytes
                .get(i + 1..i + 3)
                .and_then(|h| std::str::from_utf8(h).ok())
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    // SAFETY: on Unix any byte sequence is a valid OsStr.
    Some(PathBuf::from(unsafe {
        std::ffi::OsString::from_encoded_bytes_unchecked(out)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_survive_the_handover() {
        let paths = vec![
            PathBuf::from("/music/Polar Bear/Beartown.flac"),
            PathBuf::from("/music/ünïcödé/01 – x.flac"),
        ];
        assert_eq!(decode(&encode(&paths)[..]), paths);
    }

    #[test]
    fn a_path_with_a_newline_is_left_out() {
        let paths = vec![PathBuf::from("/a\nb.flac"), PathBuf::from("/c.flac")];
        assert_eq!(decode(&encode(&paths)[..]), vec![PathBuf::from("/c.flac")]);
    }

    #[test]
    fn one_configuration_is_one_name() {
        let a = name_of(Path::new("/home/x/.config/koan"));
        assert_eq!(a, name_of(Path::new("/home/x/.config/koan")));
        assert_ne!(a, name_of(Path::new("/tmp/koan-test")));
        // Short enough for sun_path (104 bytes on macOS) under any runtime dir.
        assert_eq!(a.len(), "koan-gui-".len() + 16);
    }

    #[test]
    fn file_urls_are_paths_and_the_rest_are_links() {
        let sorted = sort_urls(vec![
            "file:///Users/x/Music/A%20B.flac".into(),
            "https://koan.rocks/invite/abc".into(),
            "file://localhost/tmp/c.flac".into(),
        ]);
        assert_eq!(
            sorted,
            vec![
                Opened::Paths(vec![
                    PathBuf::from("/Users/x/Music/A B.flac"),
                    PathBuf::from("/tmp/c.flac"),
                ]),
                Opened::Links(vec!["https://koan.rocks/invite/abc".into()]),
            ]
        );
    }

    #[test]
    fn a_second_launch_reaches_the_first() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("s.sock");
        let listener = bind_private(&socket).unwrap();
        let mode = std::fs::metadata(&socket).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let paths = vec![PathBuf::from("/a.flac")];
        let sent = paths.clone();
        let lock_path = dir.path().join("lock");
        std::fs::write(&lock_path, socket.as_os_str().as_encoded_bytes()).unwrap();
        let handing = std::thread::spawn(move || {
            let mut lock = OpenOptions::new().read(true).open(lock_path).unwrap();
            hand(&mut lock, &sent)
        });
        let (stream, _) = listener.accept().unwrap();
        assert_eq!(decode(stream), paths);
        handing.join().unwrap().unwrap();
    }
}
