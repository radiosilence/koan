//! EQ profiles kept everywhere: synced through the account's kōan server to
//! every device signed in to it, gated on `koanDspProfiles`.
//!
//! A profile travels whole as a [`SyncDoc`], everything but which outputs use
//! it, with the files in its folder (impulse responses, a routing `.cfg`,
//! AutoEQ's measurement) named by content. The last edit wins per profile,
//! by when it was made, so an edit made offline still counts from when it
//! was made. A profile kept on this device alone is never sent, and making
//! one so deletes it from the account's other devices.
//!
//! A device reads what changed after its cursor on linking, on signing in,
//! and when the server says the profiles moved; it sends what changed here
//! after each edit. Nothing polls.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::{self, Config, DspProfile, DspScope};
use crate::db::connection::Database;
use crate::db::queries::dsp as rows;
use crate::remote::client::{KoanDspProfiles, KoanDspSaved, SubsonicClient, SubsonicError};

/// The most one file may hold: a stereo impulse response at 384 kHz runs to
/// a few MB.
pub const MAX_FILE: u64 = 32 << 20;
/// The most an account's files may hold together.
pub const MAX_ACCOUNT: u64 = 256 << 20;
/// The most a profile's document may hold, files aside.
pub const MAX_DOC: usize = 512 << 10;
/// The most files one profile may name.
pub const MAX_FILES: usize = 64;
/// The most profiles a server keeps for an account, deleted ones included.
pub const MAX_PROFILES: i64 = 1024;
/// The most outputs an account may turn AutoEQ down for.
pub const MAX_DISMISSED: i64 = 1024;

/// A profile as it travels: as configured, without the outputs that use it
/// or where it is kept, with its impulse responses named by file and every
/// file in its folder by content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncDoc {
    pub profile: DspProfile,
    pub files: Vec<SyncFile>,
    /// The profile's fields a newer kōan wrote that this one does not know:
    /// kept and written back, so a server or a device in between loses none.
    #[serde(skip)]
    pub unknown: serde_json::Map<String, serde_json::Value>,
}

/// A document as written, its profile's unknown fields set apart.
#[derive(Deserialize)]
struct Wire {
    profile: WireProfile,
    files: Vec<SyncFile>,
}

#[derive(Deserialize)]
struct WireProfile {
    #[serde(flatten)]
    known: DspProfile,
    #[serde(flatten)]
    unknown: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncFile {
    pub name: String,
    pub sha256: String,
    pub size: u64,
}

impl SyncDoc {
    /// Read and check a document from another device or the server.
    /// The profile is brought within `config::dsp_bounds`, and stripped of
    /// what is each device's own: which outputs use it, where it is kept,
    /// its uid, what it was imported from. A server keeps it so, and a
    /// device applies it so.
    pub fn parse(json: &str) -> Result<Self, String> {
        if json.len() > MAX_DOC {
            return Err(format!("An EQ may hold at most {} KB", MAX_DOC >> 10));
        }
        let wire: Wire = serde_json::from_str(json).map_err(|e| format!("Not an EQ: {e}"))?;
        let mut doc = Self {
            profile: wire.profile.known,
            files: wire.files,
            unknown: wire.profile.unknown,
        };
        doc.profile.sanitize();
        doc.profile.devices.clear();
        doc.profile.scope = None;
        doc.profile.uid = None;
        doc.profile.source.clear();
        doc.check()?;
        Ok(doc)
    }

    /// What a server and a device both refuse: files with names that could
    /// leave the profile's folder, sizes past the caps, impulse responses
    /// that are not among the files.
    pub fn check(&self) -> Result<(), String> {
        if self.profile.name.trim().is_empty() {
            return Err("It needs a name".into());
        }
        if self.files.len() > MAX_FILES {
            return Err(format!("An EQ may hold at most {MAX_FILES} files"));
        }
        let mut names = HashSet::new();
        for f in &self.files {
            if !safe_name(&f.name) {
                return Err(format!("{:?} is not a file name an EQ may hold", f.name));
            }
            if !names.insert(f.name.as_str()) {
                return Err(format!("{} is named twice", f.name));
            }
            if !is_sha256(&f.sha256) {
                return Err(format!("{} has no SHA-256", f.name));
            }
            if f.size > MAX_FILE {
                return Err(too_large(&f.name));
            }
        }
        if self.size() > MAX_ACCOUNT {
            return Err(format!(
                "Its files come to more than the {} MB an account keeps",
                MAX_ACCOUNT >> 20
            ));
        }
        for ir in &self.profile.impulses {
            let named = ir.to_str().is_some_and(|n| names.contains(n));
            if !named {
                return Err(format!("{} is not among its files", ir.display()));
            }
        }
        Ok(())
    }

    /// Its files' bytes together, each content counted once.
    pub fn size(&self) -> u64 {
        let mut seen = HashSet::new();
        self.files
            .iter()
            .filter(|f| seen.insert(&f.sha256))
            .map(|f| f.size)
            .sum()
    }

    /// As it travels: what this kōan knows, with the fields a newer one
    /// wrote, laid out the same way whether there are any.
    pub fn json(&self) -> String {
        let mut value = serde_json::to_value(self).expect("a profile serialises");
        if let Some(profile) = value.get_mut("profile").and_then(|p| p.as_object_mut()) {
            for (key, field) in &self.unknown {
                profile.entry(key.clone()).or_insert_with(|| field.clone());
            }
        }
        value.to_string()
    }

    /// What tells two versions apart: the fields this kōan knows, which are
    /// all a device keeps. A field from a newer kōan is never kept here, so
    /// counting it would make every copy taken read as edited here, and be
    /// sent back without it.
    pub fn hash(&self) -> String {
        sha256_hex(
            serde_json::to_string(self)
                .expect("a profile serialises")
                .as_bytes(),
        )
    }
}

fn too_large(name: &str) -> String {
    format!(
        "{name} is larger than the {} MB a file may be",
        MAX_FILE >> 20
    )
}

fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('.')
        && !name.contains(['/', '\\', '\0'])
}

pub fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `profile` as it travels, and where each of its files is here. Refused,
/// with the reason, for one that cannot travel: a response kept outside its
/// folder, or files past the caps.
pub fn doc_of(profile: &DspProfile) -> Result<(SyncDoc, HashMap<String, PathBuf>), String> {
    let base = config::config_dir();
    let dir = crate::audio::dsp::profiles::dir(&profile.name);
    // A folder two profiles' names map to holds both their files; sending
    // it would send the other's, which may be kept here alone.
    if let Some(other) = Config::cached()
        .dsp
        .profiles
        .iter()
        .find(|p| p.name != profile.name && crate::audio::dsp::profiles::dir(&p.name) == dir)
    {
        return Err(format!(
            "{} shares its folder with {}; rename one of them to sync it",
            profile.name, other.name
        ));
    }
    let mut files = Vec::new();
    let mut paths = HashMap::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !safe_name(&name) || !entry.file_type().is_ok_and(|t| t.is_file()) {
                continue;
            }
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            if size > MAX_FILE {
                return Err(too_large(&name));
            }
            let bytes = std::fs::read(entry.path()).map_err(|e| format!("{name}: {e}"))?;
            let sha256 = sha256_hex(&bytes);
            paths.insert(sha256.clone(), entry.path());
            files.push(SyncFile { name, sha256, size });
        }
    }
    let mut travelling = profile.clone();
    travelling.devices.clear();
    travelling.scope = None;
    travelling.uid = None;
    travelling.source.clear();
    travelling.origin = profile.origin.clone().or_else(|| Some(this_device()));
    travelling.impulses = profile
        .impulses
        .iter()
        .map(|path| {
            let full = if path.is_absolute() {
                path.clone()
            } else {
                base.join(path)
            };
            match (full.parent(), full.file_name()) {
                (Some(parent), Some(name)) if parent == dir => Ok(PathBuf::from(name)),
                _ => Err(format!(
                    "{} is kept outside {}'s folder, so it cannot be sent",
                    path.display(),
                    profile.name
                )),
            }
        })
        .collect::<Result<_, _>>()?;
    let doc = SyncDoc {
        profile: travelling,
        files,
        unknown: Default::default(),
    };
    doc.check()?;
    Ok((doc, paths))
}

/// This device's name, as the apps give it when the engine starts, for
/// telling apart two profiles of one name that came from different devices.
static DEVICE_NAME: parking_lot::RwLock<Option<String>> = parking_lot::RwLock::new(None);

pub fn set_device_name(name: Option<String>) {
    *DEVICE_NAME.write() = name.filter(|n| !n.trim().is_empty());
}

fn this_device() -> String {
    DEVICE_NAME
        .read()
        .clone()
        .unwrap_or_else(|| crate::remote::link::LinkIdentity::this_device(None).name)
}

/// Whether two profiles would sound the same, whatever else differs: their
/// names, where they are kept, which device made them, when. Gains, the
/// preamp and Q match within what no one hears (±0.05 dB, ±0.5%), and
/// frequencies within 0.1%. Parametric bands on the same channels commute,
/// so their order between delays, mixes and graphic curves does not count;
/// those, and the order around them, do. Files match by content, impulse
/// responses by the content of the file each names. Layers match when the
/// profiles they name, found by `member`, sound the same in turn.
pub fn sounds_same(a: &SyncDoc, b: &SyncDoc, member: &dyn Fn(&str) -> Option<SyncDoc>) -> bool {
    same(a, b, member, 0)
}

fn same(a: &SyncDoc, b: &SyncDoc, member: &dyn Fn(&str) -> Option<SyncDoc>, depth: usize) -> bool {
    use crate::config::DspFilter;
    let (pa, pb) = (&a.profile, &b.profile);
    let close =
        |x: f64, y: f64, abs: f64, rel: f64| (x - y).abs() <= abs.max(rel * x.abs().max(y.abs()));
    let db = |x: f64, y: f64| close(x, y, 0.05, 0.0);
    let preamp = match (pa.preamp_db, pb.preamp_db) {
        (Some(x), Some(y)) => db(x, y),
        (None, None) => true,
        _ => false,
    };
    // What the correction aims at, and whether its layers play together or
    // one at a time, change the sound as much as any band.
    if !preamp
        || pa.target != pb.target
        || pa.measurement != pb.measurement
        || pa.group != pb.group
        || pa.tuned_for != pb.tuned_for
    {
        return false;
    }
    let files = |d: &SyncDoc| {
        let mut h: Vec<String> = d.files.iter().map(|f| f.sha256.clone()).collect();
        h.sort_unstable();
        h
    };
    if files(a) != files(b) {
        return false;
    }
    let named = |d: &SyncDoc| -> Vec<String> {
        d.profile
            .impulses
            .iter()
            .filter_map(|ir| {
                d.files
                    .iter()
                    .find(|f| Some(f.name.as_str()) == ir.to_str())
            })
            .map(|f| f.sha256.clone())
            .collect()
    };
    if named(a) != named(b) {
        return false;
    }
    // Filters in runs between those whose order counts; each run's bands
    // sorted, since bands on the same channels commute.
    let runs = |filters: &[DspFilter]| -> Vec<Vec<DspFilter>> {
        let mut out = vec![Vec::new()];
        for f in filters {
            if matches!(f, DspFilter::Band(_)) {
                out.last_mut().expect("one at least").push(f.clone());
            } else {
                out.push(vec![f.clone()]);
                out.push(Vec::new());
            }
        }
        for run in &mut out {
            run.sort_by(|x, y| match (x, y) {
                (DspFilter::Band(x), DspFilter::Band(y)) => (&x.channels, x.freq)
                    .partial_cmp(&(&y.channels, y.freq))
                    .unwrap_or(std::cmp::Ordering::Equal),
                _ => std::cmp::Ordering::Equal,
            });
        }
        out.retain(|r| !r.is_empty());
        out
    };
    let alike = |x: &DspFilter, y: &DspFilter| match (x, y) {
        (DspFilter::Band(x), DspFilter::Band(y)) => {
            x.kind == y.kind
                && x.channels == y.channels
                && close(x.freq, y.freq, 0.0, 0.001)
                && db(x.gain_db, y.gain_db)
                && close(x.q, y.q, 0.0, 0.005)
        }
        (DspFilter::Delay(x), DspFilter::Delay(y)) => {
            x.channels == y.channels
                && close(x.ms, y.ms, 0.001, 0.001)
                && close(x.samples, y.samples, 0.5, 0.0)
                && x.subsample == y.subsample
        }
        (DspFilter::Mix(x), DspFilter::Mix(y)) => {
            x.outputs.len() == y.outputs.len()
                && x.outputs.iter().zip(&y.outputs).all(|(p, q)| {
                    p.len() == q.len()
                        && p.iter()
                            .zip(q)
                            .all(|((c, g), (d, h))| c == d && close(*g, *h, 0.001, 0.005))
                })
        }
        (DspFilter::Graphic(x), DspFilter::Graphic(y)) => {
            x.channels == y.channels
                && x.points.len() == y.points.len()
                && x.points
                    .iter()
                    .zip(&y.points)
                    .all(|((f, g), (e, h))| close(*f, *e, 0.0, 0.001) && db(*g, *h))
        }
        _ => false,
    };
    let (ra, rb) = (runs(&pa.filters), runs(&pb.filters));
    let filters = ra.len() == rb.len()
        && ra
            .iter()
            .zip(&rb)
            .all(|(x, y)| x.len() == y.len() && x.iter().zip(y).all(|(f, g)| alike(f, g)));
    if !filters || pa.layers.len() != pb.layers.len() {
        return false;
    }
    pa.layers.iter().zip(&pb.layers).all(|(x, y)| {
        x.on == y.on
            && (x.profile == y.profile
                || depth < 8
                    && match (member(&x.profile), member(&y.profile)) {
                        (Some(m), Some(n)) => same(&m, &n, member, depth + 1),
                        _ => false,
                    })
    })
}

/// The server, as syncing uses it: what tests stand in for.
pub trait Remote {
    fn changes(&self, since: i64) -> Result<KoanDspProfiles, SubsonicError>;
    fn save(&self, uid: &str, edited_at: i64, doc: &str) -> Result<KoanDspSaved, SubsonicError>;
    fn delete(&self, uid: &str, edited_at: i64) -> Result<KoanDspSaved, SubsonicError>;
    fn dismiss(&self, output: &str) -> Result<(), SubsonicError>;
    fn file(&self, sha256: &str) -> Result<Vec<u8>, SubsonicError>;
    fn upload(&self, sha256: &str, data: Vec<u8>) -> Result<(), SubsonicError>;
}

impl Remote for SubsonicClient {
    fn changes(&self, since: i64) -> Result<KoanDspProfiles, SubsonicError> {
        self.koan_dsp_profiles(since)
    }
    fn save(&self, uid: &str, edited_at: i64, doc: &str) -> Result<KoanDspSaved, SubsonicError> {
        self.koan_dsp_profile_save(uid, edited_at, doc)
    }
    fn delete(&self, uid: &str, edited_at: i64) -> Result<KoanDspSaved, SubsonicError> {
        self.koan_dsp_profile_delete(uid, edited_at)
    }
    fn dismiss(&self, output: &str) -> Result<(), SubsonicError> {
        self.koan_dsp_dismiss(output)
    }
    fn file(&self, sha256: &str) -> Result<Vec<u8>, SubsonicError> {
        self.koan_dsp_file(sha256)
    }
    fn upload(&self, sha256: &str, data: Vec<u8>) -> Result<(), SubsonicError> {
        self.koan_dsp_upload(sha256, data)
    }
}

/// What a sync did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DspSync {
    /// Profiles taken from other devices, deletions included.
    pub applied: usize,
    /// Profiles sent, deletions included.
    pub sent: usize,
}

impl DspSync {
    /// Whether this device's profiles moved, so the player reloads them.
    pub fn changed(&self) -> bool {
        self.applied > 0
    }
}

/// One sync at a time: two reading the same changes would each apply them.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

thread_local! {
    /// Set while a sync writes the profiles, so its own writes are not taken
    /// for edits to send.
    static APPLYING: Cell<bool> = const { Cell::new(false) };
}

/// Sync with the server signed in to, as a device does when told the
/// profiles moved.
pub fn sync(db: &Database) -> DspSync {
    let cfg = Config::load().unwrap_or_default();
    let Some(client) = crate::helpers::subsonic_client(&cfg) else {
        return DspSync::default();
    };
    reconcile(db, &client, &cfg.remote.url)
}

/// Set by a process that would exit before a background sync ran, such as
/// the CLI: [`changed`] notes an edit in `PENDING` instead, and [`flush`]
/// sends it.
static DEFERRED: AtomicBool = AtomicBool::new(false);
static PENDING: AtomicBool = AtomicBool::new(false);

/// From now on, edits wait for [`flush`] rather than syncing in the
/// background.
pub fn defer() {
    DEFERRED.store(true, Ordering::Relaxed);
}

/// Sync now if an edit was made since [`defer`]. None where nothing was
/// edited; an error where the server could not be reached or refused.
pub fn flush(db: &Database) -> Option<Result<DspSync, String>> {
    if !PENDING.swap(false, Ordering::Relaxed) {
        return None;
    }
    let cfg = Config::load().unwrap_or_default();
    let client = crate::helpers::subsonic_client(&cfg)?;
    let offers = crate::remote::profile::for_auth(client.auth())
        .is_some_and(|p| p.offers(crate::remote::profile::DSP_PROFILES));
    if !offers {
        return Some(Ok(DspSync::default()));
    }
    Some(try_run(db, client.as_ref(), &cfg.remote.url).map_err(|e| e.to_string()))
}

/// Part of every sync with the server at `url`: read what changed there,
/// then send what changed here. Nothing, for a server that does not keep
/// profiles.
pub fn reconcile(db: &Database, client: &SubsonicClient, url: &str) -> DspSync {
    let offers = crate::remote::profile::for_auth(client.auth())
        .is_some_and(|p| p.offers(crate::remote::profile::DSP_PROFILES));
    if !offers {
        return DspSync::default();
    }
    run(db, client, url)
}

/// [`reconcile`], given the server.
pub fn run(db: &Database, remote: &dyn Remote, url: &str) -> DspSync {
    try_run(db, remote, url).unwrap_or_else(|e| {
        log::warn!("dsp sync: {e}");
        DspSync::default()
    })
}

fn try_run(db: &Database, remote: &dyn Remote, url: &str) -> Result<DspSync, Failed> {
    let _one = ONE_AT_A_TIME.lock();
    APPLYING.with(|a| a.set(true));
    let out = sync_with(db, remote, url);
    APPLYING.with(|a| a.set(false));
    if out.as_ref().is_ok_and(DspSync::changed) {
        crate::signal::engine_changed().bump();
    }
    out
}

/// After an edit here: send it, in the background, where there is a server
/// to send it to.
pub fn changed() {
    if APPLYING.with(Cell::get) {
        return;
    }
    if DEFERRED.load(Ordering::Relaxed) {
        PENDING.store(true, Ordering::Relaxed);
        return;
    }
    let cfg = Config::cached();
    if crate::helpers::subsonic_auth(&cfg).is_none() {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("koan-dsp-sync".into())
        .spawn(|| {
            let Ok(db) = crate::db::pool::shared().get() else {
                return;
            };
            sync(&db);
        });
    if let Err(e) = spawned {
        log::warn!("dsp sync: could not start: {e}");
    }
}

/// What syncing did to `name` that its page says, if anything: a rename.
pub fn note(db: &Database, name: &str) -> Option<String> {
    let cfg = Config::cached();
    let uid = cfg
        .dsp
        .profiles
        .iter()
        .find(|p| p.name == name)?
        .uid
        .clone()?;
    rows::local_edits(&db.conn).ok()?.remove(&uid)?.note
}

/// Why the server refused to keep `name`, if it did.
pub fn refusal(db: &Database, name: &str) -> Option<String> {
    let cfg = Config::cached();
    let uid = cfg
        .dsp
        .profiles
        .iter()
        .find(|p| p.name == name)?
        .uid
        .clone()?;
    rows::local_edits(&db.conn).ok()?.remove(&uid)?.refused
}

#[derive(Debug, thiserror::Error)]
enum Failed {
    #[error("{0}")]
    Db(#[from] crate::db::connection::DbError),
    #[error("{0}")]
    Remote(#[from] SubsonicError),
    #[error("{0}")]
    Config(#[from] crate::config::ConfigError),
}

/// A profile kept everywhere, as it stands here.
struct Local {
    doc: SyncDoc,
    hash: String,
    paths: HashMap<String, PathBuf>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

fn sync_with(db: &Database, remote: &dyn Remote, url: &str) -> Result<DspSync, Failed> {
    let mut out = DspSync::default();
    first_sync(db)?;
    let mut locals = observe(db)?;
    let mut dismissed = pull(db, remote, url, &mut locals, &mut out)?;
    if push(db, remote, url, &locals, &dismissed, &mut out)? {
        // The server kept a copy edited later than one sent: take it.
        locals = observe(db)?;
        dismissed = pull(db, remote, url, &mut locals, &mut out)?;
    }
    let _ = dismissed;
    Ok(out)
}

/// The first time this device syncs, what it already had stays on it unless
/// it came from AutoEQ: nothing leaves a device unless it was made to travel
/// or chosen to.
fn first_sync(db: &Database) -> Result<(), Failed> {
    let first = rows::local_edits(&db.conn)?.is_empty()
        && db
            .conn
            .query_row("SELECT COUNT(*) FROM dsp_sync_cursor", [], |r| {
                r.get::<_, i64>(0)
            })
            .map_err(crate::db::connection::DbError::from)?
            == 0;
    if !first {
        return Ok(());
    }
    let cfg = Config::cached();
    let pin: Vec<String> = cfg
        .dsp
        .profiles
        .iter()
        .filter(|p| {
            p.scope.is_none()
                && p.target.is_none()
                && p.measurement.is_none()
                && p.layers.is_empty()
        })
        .map(|p| p.name.clone())
        .collect();
    if !pin.is_empty() {
        Config::persist(|c| {
            for p in c.dsp.profiles.iter_mut().filter(|p| pin.contains(&p.name)) {
                p.scope = Some(DspScope::Device);
            }
        })?;
    }
    Ok(())
}

/// Every profile kept everywhere, each given a uid if it lacks one, with an
/// edit recorded for each whose content moved since it was last seen.
fn observe(db: &Database) -> Result<HashMap<String, Local>, Failed> {
    let cfg = Config::cached();
    let all = cfg.dsp.profiles.clone();
    let everywhere: Vec<&DspProfile> = all
        .iter()
        .filter(|p| crate::audio::dsp::profiles::scope(p, &all) == DspScope::Everywhere)
        .collect();
    let unnamed: Vec<String> = everywhere
        .iter()
        .filter(|p| p.uid.is_none())
        .map(|p| p.name.clone())
        .collect();
    if !unnamed.is_empty() {
        Config::persist(|c| {
            for p in c
                .dsp
                .profiles
                .iter_mut()
                .filter(|p| unnamed.contains(&p.name))
            {
                p.uid = Some(uuid::Uuid::now_v7().to_string());
            }
        })?;
        return observe(db);
    }
    let seen = rows::local_edits(&db.conn)?;
    let mut locals = HashMap::new();
    for p in everywhere {
        let uid = p.uid.clone().expect("given one above");
        match doc_of(p) {
            Ok((doc, paths)) => {
                let hash = doc.hash();
                if seen.get(&uid).is_none_or(|s| s.hash != hash) {
                    rows::set_local_edit(&db.conn, &uid, &hash, now_ms())?;
                }
                locals.insert(uid, Local { doc, hash, paths });
            }
            Err(why) => {
                if seen
                    .get(&uid)
                    .is_none_or(|s| s.refused.as_deref() != Some(&why))
                {
                    rows::set_local_edit(&db.conn, &uid, "", now_ms())?;
                    rows::set_refused(&db.conn, &uid, Some(&why))?;
                }
            }
        }
    }
    Ok(locals)
}

/// Take what changed on the server after this device's cursor. Stops before
/// a profile whose files cannot be fetched yet, to read it again next time.
/// Returns the outputs the account dismissed AutoEQ for.
fn pull(
    db: &Database,
    remote: &dyn Remote,
    url: &str,
    locals: &mut HashMap<String, Local>,
    out: &mut DspSync,
) -> Result<HashSet<String>, Failed> {
    let cursor = rows::sync_cursor(&db.conn, url)?;
    let page = remote.changes(cursor)?;
    let mut synced = rows::synced(&db.conn, url)?;
    let edits = rows::local_edits(&db.conn)?;
    let mut held = false;
    for row in &page.profile {
        // This device's own change, sent after the cursor was last moved:
        // already here, or deleted here since, which is sent next.
        if synced.get(&row.uid).is_some_and(|(rev, _)| *rev >= row.rev) {
            rows::set_sync_cursor(&db.conn, url, row.rev)?;
            continue;
        }
        // The same profile made here before this device first synced, such
        // as one headphone installed from AutoEQ on two devices: one profile.
        if let Some(doc) = row.doc.as_deref().and_then(|j| SyncDoc::parse(j).ok()) {
            let folder = crate::audio::dsp::profiles::dir(&doc.profile.name);
            let member = |name: &str| {
                let cfg = Config::cached();
                cfg.dsp
                    .profiles
                    .iter()
                    .find(|p| p.name == name)
                    .and_then(|p| doc_of(p).ok())
                    .map(|(d, _)| d)
            };
            let twin = locals
                .iter()
                .find(|(uid, l)| {
                    **uid != row.uid
                        && !synced.contains_key(*uid)
                        && crate::audio::dsp::profiles::dir(&l.doc.profile.name) == folder
                        && sounds_same(&l.doc, &doc, &member)
                })
                .map(|(uid, _)| uid.clone());
            if let Some(twin) = twin {
                Config::persist(|c| {
                    for p in c
                        .dsp
                        .profiles
                        .iter_mut()
                        .filter(|p| p.uid.as_deref() == Some(&twin))
                    {
                        p.uid = Some(row.uid.clone());
                    }
                })?;
                rows::forget_local(&db.conn, &twin)?;
                let l = locals.remove(&twin).expect("found above");
                locals.insert(row.uid.clone(), l);
            }
        }
        let local = locals.get(&row.uid);
        let dirty = local.is_some_and(|l| synced.get(&row.uid).is_none_or(|(_, h)| *h != l.hash));
        let edited_here = edits.get(&row.uid).map_or(0, |e| e.edited_at);
        if dirty && edited_here > row.edited_at {
            // Edited here since: this device's copy is sent instead.
        } else if let Some(json) = &row.doc {
            let doc = match SyncDoc::parse(json) {
                Ok(doc) => doc,
                Err(e) => {
                    log::warn!("dsp sync: skipping {}: {e}", row.uid);
                    rows::set_sync_cursor(&db.conn, url, row.rev)?;
                    continue;
                }
            };
            let have: HashSet<&str> = local
                .map(|l| l.paths.keys().map(String::as_str).collect())
                .unwrap_or_default();
            let mut fetched = Vec::new();
            for f in doc
                .files
                .iter()
                .filter(|f| !have.contains(f.sha256.as_str()))
            {
                match remote.file(&f.sha256) {
                    Ok(bytes) if sha256_hex(&bytes) == f.sha256 => {
                        fetched.push((f.sha256.clone(), bytes))
                    }
                    other => {
                        log::info!(
                            "dsp sync: {} not fetched yet ({:?}); holding the cursor",
                            f.name,
                            other.err()
                        );
                        held = true;
                        break;
                    }
                }
            }
            if held {
                break;
            }
            let reused: HashMap<String, PathBuf> =
                local.map(|l| l.paths.clone()).unwrap_or_default();
            for (renamed, old, new) in adopt(&row.uid, &doc, fetched, &reused, &synced)? {
                let why = format!(
                    "Renamed from “{old}”: it met a different EQ of that name from another device, so each is named for where it came from"
                );
                log::info!("dsp sync: {old} is now {new}");
                if let Some(uid) = renamed {
                    rows::set_note(&db.conn, &uid, &why)?;
                }
            }
            let hash = doc.hash();
            rows::set_synced(&db.conn, url, &row.uid, row.rev, &hash)?;
            rows::set_local_edit(&db.conn, &row.uid, &hash, row.edited_at)?;
            synced.insert(row.uid.clone(), (row.rev, hash));
            out.applied += 1;
        } else {
            drop_profile(&row.uid)?;
            rows::forget_synced(&db.conn, url, &row.uid)?;
            rows::forget_local(&db.conn, &row.uid)?;
            synced.remove(&row.uid);
            locals.remove(&row.uid);
            out.applied += 1;
        }
        rows::set_sync_cursor(&db.conn, url, row.rev)?;
    }
    if !held {
        rows::set_sync_cursor(&db.conn, url, page.cursor)?;
    }
    let dismissed: HashSet<String> = page.dismissed.into_iter().map(|d| d.output).collect();
    let cfg = Config::cached();
    let missing: Vec<String> = dismissed
        .iter()
        .filter(|d| !cfg.dsp.autoeq_dismissed.contains(d))
        .cloned()
        .collect();
    if !missing.is_empty() {
        Config::persist(|c| c.dsp.autoeq_dismissed.extend(missing))?;
    }
    if out.applied > 0 {
        *locals = observe(db)?;
    }
    Ok(dismissed)
}

/// Send each profile kept everywhere that changed here since it was last
/// synced, and delete on the server those that went from here. Whether the
/// server kept a later copy of any, which a pull then takes.
fn push(
    db: &Database,
    remote: &dyn Remote,
    url: &str,
    locals: &HashMap<String, Local>,
    dismissed: &HashSet<String>,
    out: &mut DspSync,
) -> Result<bool, Failed> {
    let synced = rows::synced(&db.conn, url)?;
    let edits = rows::local_edits(&db.conn)?;
    // Gone from here, or kept here alone now: gone everywhere else too.
    // Deletions go first, so a profile deleted and made again under its
    // name reaches other devices after the one it replaces has left.
    for uid in synced.keys().filter(|u| !locals.contains_key(*u)) {
        let refused = edits.get(uid).is_some_and(|e| e.refused.is_some());
        if refused {
            continue;
        }
        remote.delete(uid, now_ms())?;
        rows::forget_synced(&db.conn, url, uid)?;
        rows::forget_local(&db.conn, uid)?;
        out.sent += 1;
    }
    // What was kept everywhere and no longer is, and was never sent or has
    // now been deleted: nothing is left to track. One kept everywhere that
    // cannot be sent is not in `locals`, and keeps its refusal.
    let cfg = Config::cached();
    let kept: HashSet<&str> = cfg
        .dsp
        .profiles
        .iter()
        .filter(|p| {
            crate::audio::dsp::profiles::scope(p, &cfg.dsp.profiles) == DspScope::Everywhere
        })
        .filter_map(|p| p.uid.as_deref())
        .collect();
    for uid in edits
        .keys()
        .filter(|u| !kept.contains(u.as_str()) && !synced.contains_key(*u))
    {
        rows::forget_local(&db.conn, uid)?;
    }
    let mut kept_later = false;
    let mut uids: Vec<&String> = locals.keys().collect();
    uids.sort();
    for uid in uids {
        let local = &locals[uid];
        if synced.get(uid).is_some_and(|(_, h)| *h == local.hash) {
            continue;
        }
        // Never synced from here, or synced and then deleted on the server
        // when it was kept here alone: sending it now is the edit.
        let edited_at = match synced.get(uid) {
            Some(_) => edits.get(uid).map_or_else(now_ms, |e| e.edited_at),
            None => now_ms(),
        };
        let saved = match remote.save(uid, edited_at, &local.doc.json()) {
            Ok(saved) => saved,
            Err(SubsonicError::Api { message, .. }) => {
                rows::set_refused(&db.conn, uid, Some(&message))?;
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        if !saved.stored {
            kept_later = true;
            continue;
        }
        for m in &saved.missing {
            let Some(path) = local.paths.get(&m.sha256) else {
                continue;
            };
            let bytes = std::fs::read(path)?;
            remote.upload(&m.sha256, bytes)?;
        }
        rows::set_synced(&db.conn, url, uid, saved.rev, &local.hash)?;
        rows::set_refused(&db.conn, uid, None)?;
        out.sent += 1;
    }
    let cfg = Config::cached();
    for output in cfg
        .dsp
        .autoeq_dismissed
        .iter()
        .filter(|d| !dismissed.contains(*d))
    {
        remote.dismiss(output)?;
    }
    Ok(kept_later)
}

impl From<std::io::Error> for Failed {
    fn from(e: std::io::Error) -> Self {
        Self::Remote(SubsonicError::Io(e))
    }
}

/// Take `doc` as profile `uid`: update the one here with that uid, or add
/// it. Another profile here holding its name, not yet synced, gives the name
/// up and is renamed. Its folder is made to hold exactly the doc's files:
/// `fetched` by content, the rest from `reused`.
/// Renamed profiles, each by uid where it has one: its old name and new.
type Renamed = Vec<(Option<String>, String, String)>;

fn adopt(
    uid: &str,
    doc: &SyncDoc,
    fetched: Vec<(String, Vec<u8>)>,
    reused: &HashMap<String, PathBuf>,
    synced: &HashMap<String, (i64, String)>,
) -> Result<Renamed, Failed> {
    use crate::audio::dsp::profiles;
    let cfg = Config::cached();
    let name = doc.profile.name.clone();
    let current = cfg
        .dsp
        .profiles
        .iter()
        .find(|p| p.uid.as_deref() == Some(uid));
    // Another profile whose folder this one's name maps to gives the name
    // up, so its files are never written over: folders are keyed by the
    // name's slug, not the name.
    let mut name = name;
    let mut renamed = Vec::new();
    let folder = profiles::dir(&name);
    if let Some(holder) = cfg
        .dsp
        .profiles
        .iter()
        .find(|p| profiles::dir(&p.name) == folder && p.uid.as_deref() != Some(uid))
        && holder.uid.as_ref().is_some_and(|u| synced.contains_key(u))
    {
        // Already synced: a later row on this page renames or deletes it, as
        // with a rename chain or a profile deleted and made again. Until then
        // it steps aside.
        let free = free_name(&cfg.dsp.profiles, &holder.name);
        profiles::rename(&holder.name, &free).map_err(io_failed)?;
    } else if let Some(holder) = cfg
        .dsp
        .profiles
        .iter()
        .find(|p| profiles::dir(&p.name) == folder && p.uid.as_deref() != Some(uid))
    {
        // Two different profiles of one name, made apart before either
        // synced: each is named for the device it came from, so neither
        // looks like the other.
        let here = this_device();
        let mine = by_device(&cfg.dsp.profiles, &holder.name, &here);
        profiles::rename(&holder.name, &mine).map_err(io_failed)?;
        renamed.push((holder.uid.clone(), holder.name.clone(), mine));
        let theirs = match doc.profile.origin.as_deref() {
            Some(origin) if origin != here => format!("{name} ({origin})"),
            _ => free_name(&Config::cached().dsp.profiles, &name),
        };
        let theirs = if Config::cached()
            .dsp
            .profiles
            .iter()
            .any(|p| p.name == theirs)
        {
            free_name(&Config::cached().dsp.profiles, &theirs)
        } else {
            theirs
        };
        renamed.push((Some(uid.to_owned()), name.clone(), theirs.clone()));
        name = theirs;
    }
    if let Some(current) = current
        && current.name != name
    {
        profiles::rename(&current.name, &name).map_err(io_failed)?;
    }

    // The folder: every file the doc names, nothing else.
    let dir = profiles::dir(&name);
    let staged = dir.with_extension("incoming");
    let _ = std::fs::remove_dir_all(&staged);
    std::fs::create_dir_all(&staged)?;
    let fetched: HashMap<String, Vec<u8>> = fetched.into_iter().collect();
    for f in &doc.files {
        let target = staged.join(&f.name);
        match fetched.get(&f.sha256) {
            Some(bytes) => std::fs::write(&target, bytes)?,
            None => {
                let from = reused
                    .get(&f.sha256)
                    .ok_or_else(|| io_failed(format!("{} has not arrived", f.name)))?;
                std::fs::copy(from, &target)?;
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    if doc.files.is_empty() {
        let _ = std::fs::remove_dir_all(&staged);
    } else {
        std::fs::rename(&staged, &dir)?;
    }

    let relative = Path::new("dsp").join(dir.file_name().expect("a profile's folder has a name"));
    Config::persist(|c| {
        let mut incoming = doc.profile.clone();
        incoming.name = name.clone();
        incoming.impulses = incoming.impulses.iter().map(|f| relative.join(f)).collect();
        incoming.uid = Some(uid.to_owned());
        incoming.scope = Some(DspScope::Everywhere);
        match c
            .dsp
            .profiles
            .iter_mut()
            .find(|p| p.uid.as_deref() == Some(uid))
        {
            Some(p) => {
                incoming.devices = std::mem::take(&mut p.devices);
                *p = incoming;
            }
            None => c.dsp.profiles.push(incoming),
        }
    })?;
    Ok(renamed)
}

/// `name` with `device` after it, as a profile here may be called: numbered
/// as well if even that is taken.
fn by_device(profiles: &[DspProfile], name: &str, device: &str) -> String {
    let wanted = format!("{name} ({device})");
    if profiles.iter().any(|p| p.name == wanted) {
        free_name(profiles, &wanted)
    } else {
        wanted
    }
}

/// Delete the profile here with `uid`, and its folder. A stack that played
/// it says so, as for any missing layer.
fn drop_profile(uid: &str) -> Result<(), Failed> {
    let cfg = Config::cached();
    let Some(profile) = cfg
        .dsp
        .profiles
        .iter()
        .find(|p| p.uid.as_deref() == Some(uid))
    else {
        return Ok(());
    };
    if crate::audio::dsp::profiles::scope(profile, &cfg.dsp.profiles) == DspScope::Device {
        // Kept here by choice since: the deletion was this device's own.
        return Ok(());
    }
    let dir = crate::audio::dsp::profiles::dir(&profile.name);
    let shared =
        cfg.dsp.profiles.iter().any(|p| {
            p.uid.as_deref() != Some(uid) && crate::audio::dsp::profiles::dir(&p.name) == dir
        });
    Config::persist(|c| c.dsp.profiles.retain(|p| p.uid.as_deref() != Some(uid)))?;
    if !shared {
        let _ = std::fs::remove_dir_all(dir);
    }
    Ok(())
}

/// `name` numbered, until neither the name nor its folder is taken.
fn free_name(profiles: &[DspProfile], name: &str) -> String {
    use crate::audio::dsp::profiles::dir;
    (2..)
        .map(|n| format!("{name} {n}"))
        .find(|candidate| {
            profiles
                .iter()
                .all(|p| &p.name != candidate && dir(&p.name) != dir(candidate))
        })
        .expect("some number is free")
}

fn io_failed(e: impl ToString) -> Failed {
    Failed::Remote(SubsonicError::Io(std::io::Error::other(e.to_string())))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::Path;

    use super::*;
    use crate::config::{DspFilter, DspTarget, EqFilter, EqFilterKind};
    use crate::remote::client::{KoanDspDismissed, KoanDspMissing, KoanDspProfile};

    /// The server's side, over the same queries a kōan server runs.
    struct Server {
        conn: rusqlite::Connection,
        uploads: RefCell<usize>,
    }

    const USER: i64 = 1;

    impl Server {
        fn new() -> Self {
            let conn = rusqlite::Connection::open_in_memory().unwrap();
            crate::db::schema::create_tables(&conn).unwrap();
            conn.execute(
                "INSERT INTO users (id, username, password_hash, role) VALUES (1, 'mate', 'x', 'user')",
                [],
            )
            .unwrap();
            Self {
                conn,
                uploads: RefCell::new(0),
            }
        }

        fn named(&self) -> HashMap<String, u64> {
            rows::live_docs(&self.conn, USER)
                .unwrap()
                .into_iter()
                .flat_map(|(_, json)| SyncDoc::parse(&json).unwrap().files)
                .map(|f| (f.sha256, f.size))
                .collect()
        }
    }

    fn api(message: &str) -> SubsonicError {
        SubsonicError::Api {
            code: 0,
            message: message.into(),
        }
    }

    impl Remote for Server {
        fn changes(&self, since: i64) -> Result<KoanDspProfiles, SubsonicError> {
            let (rows, cursor) = rows::changes(&self.conn, USER, since).unwrap();
            Ok(KoanDspProfiles {
                cursor,
                profile: rows
                    .into_iter()
                    .map(|r| KoanDspProfile {
                        uid: r.uid,
                        rev: r.rev,
                        edited_at: r.edited_at,
                        doc: r.doc,
                    })
                    .collect(),
                dismissed: rows::dismissed(&self.conn, USER)
                    .unwrap()
                    .into_iter()
                    .map(|output| KoanDspDismissed { output })
                    .collect(),
            })
        }
        fn save(
            &self,
            uid: &str,
            edited_at: i64,
            doc: &str,
        ) -> Result<KoanDspSaved, SubsonicError> {
            let parsed = SyncDoc::parse(doc).map_err(|e| api(&e))?;
            let saved = rows::save(&self.conn, USER, uid, edited_at, Some(doc)).unwrap();
            let held = rows::files(&self.conn, USER).unwrap();
            Ok(KoanDspSaved {
                rev: saved.rev,
                stored: saved.stored,
                missing: parsed
                    .files
                    .iter()
                    .filter(|f| saved.stored && !held.contains_key(&f.sha256))
                    .map(|f| KoanDspMissing {
                        sha256: f.sha256.clone(),
                    })
                    .collect(),
            })
        }
        fn delete(&self, uid: &str, edited_at: i64) -> Result<KoanDspSaved, SubsonicError> {
            let saved = rows::save(&self.conn, USER, uid, edited_at, None).unwrap();
            Ok(KoanDspSaved {
                rev: saved.rev,
                stored: saved.stored,
                missing: vec![],
            })
        }
        fn dismiss(&self, output: &str) -> Result<(), SubsonicError> {
            rows::dismiss(&self.conn, USER, output).unwrap();
            Ok(())
        }
        fn file(&self, sha256: &str) -> Result<Vec<u8>, SubsonicError> {
            rows::file(&self.conn, USER, sha256)
                .unwrap()
                .ok_or_else(|| api("File not found"))
        }
        fn upload(&self, sha256: &str, data: Vec<u8>) -> Result<(), SubsonicError> {
            assert_eq!(sha256_hex(&data), sha256);
            assert_eq!(self.named().get(sha256), Some(&(data.len() as u64)));
            rows::store_file(&self.conn, USER, sha256, &data).unwrap();
            *self.uploads.borrow_mut() += 1;
            Ok(())
        }
    }

    /// A device: its own config directory and database.
    struct Device {
        dir: tempfile::TempDir,
        db: Database,
        name: &'static str,
    }

    impl Device {
        fn new() -> Self {
            Self::named("Device")
        }

        fn named(name: &'static str) -> Self {
            let conn = rusqlite::Connection::open_in_memory().unwrap();
            crate::db::schema::create_tables(&conn).unwrap();
            Self {
                dir: tempfile::tempdir().unwrap(),
                db: Database { conn },
                name,
            }
        }

        /// Make this the device the config belongs to.
        fn on(&self) -> &Self {
            config::set_config_dir(self.dir.path());
            set_device_name(Some(self.name.to_owned()));
            self
        }

        fn sync(&self, server: &Server) -> DspSync {
            self.on();
            // Edits made in the same millisecond would tie.
            std::thread::sleep(std::time::Duration::from_millis(3));
            run(&self.db, server, "https://music.example")
        }

        fn profiles(&self) -> Vec<DspProfile> {
            self.on();
            Config::cached().dsp.profiles.clone()
        }

        fn profile(&self, name: &str) -> Option<DspProfile> {
            self.profiles().into_iter().find(|p| p.name == name)
        }
    }

    fn band(gain_db: f64) -> DspFilter {
        DspFilter::Band(EqFilter {
            kind: EqFilterKind::Peaking,
            freq: 1000.0,
            gain_db,
            q: 1.0,
            channels: vec![],
        })
    }

    fn headphone() -> DspProfile {
        DspProfile {
            name: "HD 650 (AutoEQ, oratory1990)".into(),
            devices: vec!["Topping E30".into()],
            filters: vec![band(3.0)],
            target: Some(DspTarget {
                made_for: "harman-over-ear-2018".into(),
                chosen: None,
            }),
            ..Default::default()
        }
    }

    /// Deferred, an edit waits for `flush` and starts no thread a CLI would
    /// exit before; with no server signed in to, `flush` takes it and
    /// sends nothing.
    #[test]
    fn deferred_edits_wait_for_flush() {
        let _guard = lock();
        let here = Device::new();
        here.on();
        defer();
        changed();
        assert!(PENDING.load(Ordering::Relaxed));
        assert!(flush(&here.db).is_none());
        assert!(!PENDING.load(Ordering::Relaxed));
        assert!(flush(&here.db).is_none());
        DEFERRED.store(false, Ordering::Relaxed);
    }

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        crate::config::tests::PERSIST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// A headphone correction goes everywhere, without the output it was set
    /// for; what a device had of its own before stays on it; an edit made
    /// later wins; a deletion reaches every device.
    #[test]
    fn profiles_follow_the_account() {
        let _guard = lock();
        let server = Server::new();
        let (a, b) = (Device::new(), Device::new());

        a.on();
        Config::persist(|c| {
            c.dsp.profiles.push(headphone());
            c.dsp.profiles.push(DspProfile {
                name: "Desk speakers".into(),
                filters: vec![band(-2.0)],
                ..Default::default()
            });
        })
        .unwrap();
        assert_eq!(a.sync(&server).sent, 1);
        assert_eq!(
            a.profile("Desk speakers").unwrap().scope,
            Some(DspScope::Device),
            "kept here: it was here before syncing"
        );

        b.sync(&server);
        let theirs = b.profile("HD 650 (AutoEQ, oratory1990)").unwrap();
        assert_eq!(theirs.filters, vec![band(3.0)]);
        assert!(
            theirs.devices.is_empty(),
            "which output uses it stays per device"
        );
        assert_eq!(
            theirs.uid,
            a.profile("HD 650 (AutoEQ, oratory1990)").unwrap().uid
        );
        assert!(b.profile("Desk speakers").is_none());

        // Edited on A (by hand: the apps leave a correction as made), then
        // renamed on B, offline: B's later edit wins.
        a.on();
        Config::persist(|c| {
            let hd = c
                .dsp
                .profiles
                .iter_mut()
                .find(|p| p.name == "HD 650 (AutoEQ, oratory1990)")
                .unwrap();
            hd.filters[0] = band(5.0);
        })
        .unwrap();
        a.sync(&server);
        std::thread::sleep(std::time::Duration::from_millis(3));
        b.on();
        crate::audio::dsp::profiles::rename("HD 650 (AutoEQ, oratory1990)", "HD 650").unwrap();
        b.sync(&server);
        a.sync(&server);
        let a_now = a.profile("HD 650").expect("renamed on A too");
        assert_eq!(a_now.filters, vec![band(3.0)], "B's copy was edited later");
        assert_eq!(
            a_now.devices,
            vec!["Topping E30".to_string()],
            "A's output kept"
        );
        assert_eq!(b.profile("HD 650").unwrap().filters, vec![band(3.0)]);

        // Deleted on B: gone from A.
        b.on();
        crate::audio::dsp::profiles::remove("HD 650").unwrap();
        b.sync(&server);
        a.sync(&server);
        assert!(a.profile("HD 650").is_none());
        assert!(a.profile("Desk speakers").is_some());
    }

    /// Files travel by content: an impulse response arrives byte for byte,
    /// under the profile's folder on the other device.
    #[test]
    fn files_travel_with_their_profile() {
        let _guard = lock();
        let server = Server::new();
        let (a, b) = (Device::new(), Device::new());
        a.on();
        let dir = crate::audio::dsp::profiles::dir("Room");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("48000.wav"), b"RIFF not really").unwrap();
        Config::persist(|c| {
            c.dsp.profiles.push(DspProfile {
                name: "Room".into(),
                impulses: vec![Path::new("dsp/room/48000.wav").into()],
                scope: Some(DspScope::Everywhere),
                ..Default::default()
            })
        })
        .unwrap();
        a.sync(&server);
        assert_eq!(*server.uploads.borrow(), 1);
        b.sync(&server);
        b.on();
        let room = b.profile("Room").unwrap();
        assert_eq!(room.impulses, vec![PathBuf::from("dsp/room/48000.wav")]);
        assert_eq!(
            std::fs::read(config::config_dir().join("dsp/room/48000.wav")).unwrap(),
            b"RIFF not really"
        );

        // Kept on A alone now: gone from B, still on A.
        a.on();
        crate::audio::dsp::profiles::set_scope("Room", DspScope::Device).unwrap();
        a.sync(&server);
        b.sync(&server);
        assert!(b.profile("Room").is_none());
        assert!(a.profile("Room").is_some());
        // Neither device tracks it as kept everywhere any more.
        assert!(rows::local_edits(&a.db.conn).unwrap().is_empty());
        assert!(rows::local_edits(&b.db.conn).unwrap().is_empty());
    }

    /// A profile kept everywhere that cannot be sent keeps its refusal, and
    /// its record, across syncs.
    #[test]
    fn an_unsendable_profile_keeps_its_refusal() {
        let _guard = lock();
        let server = Server::new();
        let a = Device::new();
        a.on();
        let dir = crate::audio::dsp::profiles::dir("Huge");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::File::create(dir.join("48000.wav"))
            .unwrap()
            .set_len(MAX_FILE + 1)
            .unwrap();
        Config::persist(|c| {
            c.dsp.profiles.push(DspProfile {
                name: "Huge".into(),
                impulses: vec![Path::new("dsp/huge/48000.wav").into()],
                scope: Some(DspScope::Everywhere),
                ..Default::default()
            })
        })
        .unwrap();
        a.sync(&server);
        a.sync(&server);
        a.on();
        assert!(refusal(&a.db, "Huge").is_some());
        assert_eq!(rows::local_edits(&a.db.conn).unwrap().len(), 1);
    }

    /// A correction built from a measurement arrives as made: its role, its
    /// measurement and target, and the measurement's file.
    #[test]
    fn a_measured_correction_travels_whole() {
        use crate::audio::dsp::profiles;
        use crate::config::{DspEar, DspRole};
        let _guard = lock();
        let server = Server::new();
        let (a, b) = (Device::new(), Device::new());
        a.on();
        let mut text = String::from("frequency,raw\n");
        for hz in crate::audio::dsp::targets::grid() {
            text.push_str(&format!("{hz:.2},{:.1}\n", 90.0 + (hz / 1000.0).log2()));
        }
        profiles::save_measured(
            "IEM",
            &text,
            DspEar::In,
            "diffuse-field-iso-11904-1",
            Default::default(),
        )
        .unwrap();
        let sent = a.profile("IEM").unwrap();
        let (doc, paths) = doc_of(&sent).unwrap();
        let back = SyncDoc::parse(&doc.json()).unwrap();
        assert_eq!(back.profile.measurement, sent.measurement);
        assert!(back.files.iter().any(|f| f.name == "measurement.csv"));
        assert!(paths.values().any(|p| p.ends_with("measurement.csv")));

        a.sync(&server);
        b.sync(&server);
        b.on();
        let got = b.profile("IEM").unwrap();
        assert_eq!(got.measurement, sent.measurement);
        assert_eq!(profiles::role(&got), DspRole::Correction);
        a.on();
        let file = std::fs::read(profiles::dir("IEM").join("measurement.csv")).unwrap();
        b.on();
        assert_eq!(
            std::fs::read(profiles::dir("IEM").join("measurement.csv")).unwrap(),
            file
        );

        // A role said on one device is said on the other.
        a.on();
        profiles::set_role("IEM", DspRole::Baked).unwrap();
        a.sync(&server);
        b.sync(&server);
        assert_eq!(b.profile("IEM").unwrap().role, Some(DspRole::Baked));
    }

    /// The same headphone installed on two devices before either synced is
    /// one profile; another holding a synced profile's name gives it up.
    /// Two devices with a profile of one name: one profile where they would
    /// sound the same, bands in another order and all; each renamed for its
    /// device where they would not, with a note saying why.
    #[test]
    fn a_name_is_shared_once() {
        let _guard = lock();
        let server = Server::new();
        let (a, b) = (Device::named("Mac Studio"), Device::named("iPhone"));
        for (d, gain) in [(&a, 1.0), (&b, 2.0)] {
            d.on();
            let mut hp = headphone();
            hp.filters = vec![
                band(3.0),
                DspFilter::Band(EqFilter {
                    kind: EqFilterKind::LowShelf,
                    freq: 100.0,
                    gain_db: 2.0 + if d.name == "iPhone" { 0.04 } else { 0.0 },
                    q: 0.7,
                    channels: vec![],
                }),
            ];
            if d.name == "iPhone" {
                hp.filters.reverse();
            }
            Config::persist(|c| {
                c.dsp.profiles.push(hp);
                c.dsp.profiles.push(DspProfile {
                    name: "Lush".into(),
                    filters: vec![band(gain)],
                    scope: Some(DspScope::Everywhere),
                    ..Default::default()
                });
            })
            .unwrap();
        }
        a.sync(&server);
        b.sync(&server);
        a.sync(&server);
        let names = |d: &Device| -> Vec<String> {
            let mut n: Vec<String> = d.profiles().into_iter().map(|p| p.name).collect();
            n.sort();
            n
        };
        let all = vec![
            "HD 650 (AutoEQ, oratory1990)",
            "Lush (Mac Studio)",
            "Lush (iPhone)",
        ];
        assert_eq!(names(&a), all);
        assert_eq!(names(&b), all);
        assert_eq!(
            a.profile("HD 650 (AutoEQ, oratory1990)").unwrap().uid,
            b.profile("HD 650 (AutoEQ, oratory1990)").unwrap().uid,
            "reordered bands, 0.04 dB apart: the same profile"
        );
        assert_eq!(b.profile("Lush (iPhone)").unwrap().filters, vec![band(2.0)]);
        assert_eq!(
            a.profile("Lush (Mac Studio)").unwrap().filters,
            vec![band(1.0)]
        );
        b.on();
        let note = note(&b.db, "Lush (iPhone)").unwrap();
        assert!(note.starts_with("Renamed from “Lush”"), "{note}");
    }

    /// A profile deleted and made again under its name before the next sync,
    /// as reinstalling an AutoEQ headphone does, keeps the name everywhere.
    #[test]
    fn a_profile_made_again_keeps_its_name() {
        use crate::audio::dsp::profiles;
        let _guard = lock();
        let server = Server::new();
        let (a, b) = (Device::named("Mac Studio"), Device::named("iPhone"));
        a.on();
        Config::persist(|c| {
            c.dsp.profiles.push(headphone());
            c.dsp.profiles.push(DspProfile {
                name: "Lush".into(),
                filters: vec![band(1.0)],
                scope: Some(DspScope::Everywhere),
                ..Default::default()
            });
        })
        .unwrap();
        a.sync(&server);
        b.sync(&server);
        a.on();
        profiles::remove("Lush").unwrap();
        profiles::remove("HD 650 (AutoEQ, oratory1990)").unwrap();
        let mut hp = headphone();
        hp.filters = vec![band(4.0)];
        Config::persist(|c| {
            c.dsp.profiles.push(hp);
            c.dsp.profiles.push(DspProfile {
                name: "Lush".into(),
                filters: vec![band(2.0)],
                scope: Some(DspScope::Everywhere),
                ..Default::default()
            });
        })
        .unwrap();
        a.sync(&server);
        b.sync(&server);
        a.sync(&server);
        for d in [&a, &b] {
            let mut names: Vec<String> = d.profiles().into_iter().map(|p| p.name).collect();
            names.sort();
            assert_eq!(
                names,
                ["HD 650 (AutoEQ, oratory1990)", "Lush"],
                "{}",
                d.name
            );
            assert_eq!(d.profile("Lush").unwrap().filters, vec![band(2.0)]);
            assert_eq!(
                d.profile("HD 650 (AutoEQ, oratory1990)").unwrap().filters,
                vec![band(4.0)]
            );
            d.on();
            assert_eq!(note(&d.db, "Lush"), None, "{}", d.name);
        }
    }

    /// Renames made in a chain, B to C and then A to B, arrive as made: the
    /// profile still called B here steps aside until its own rename lands.
    #[test]
    fn a_rename_chain_arrives_as_made() {
        use crate::audio::dsp::profiles;
        let _guard = lock();
        let server = Server::new();
        let (a, b) = (Device::named("Mac Studio"), Device::named("iPhone"));
        a.on();
        Config::persist(|c| {
            for (name, gain) in [("A", 1.0), ("B", 2.0)] {
                c.dsp.profiles.push(DspProfile {
                    name: name.into(),
                    filters: vec![band(gain)],
                    scope: Some(DspScope::Everywhere),
                    ..Default::default()
                });
            }
        })
        .unwrap();
        a.sync(&server);
        b.sync(&server);
        a.on();
        profiles::rename("B", "C").unwrap();
        profiles::rename("A", "B").unwrap();
        a.sync(&server);
        b.sync(&server);
        a.sync(&server);
        for d in [&a, &b] {
            let mut names: Vec<String> = d.profiles().into_iter().map(|p| p.name).collect();
            names.sort();
            assert_eq!(names, ["B", "C"], "{}", d.name);
            assert_eq!(d.profile("B").unwrap().filters, vec![band(1.0)]);
            assert_eq!(d.profile("C").unwrap().filters, vec![band(2.0)]);
        }
    }

    fn doc(filters: Vec<DspFilter>, files: Vec<SyncFile>) -> SyncDoc {
        SyncDoc {
            profile: DspProfile {
                name: "X".into(),
                filters,
                ..Default::default()
            },
            files,
            unknown: Default::default(),
        }
    }

    /// A field a newer kōan wrote is kept through a parse and written back,
    /// so a server or device in between loses none.
    #[test]
    fn a_newer_field_is_passed_on() {
        let json = r#"{"profile":{"name":"Warm","filters":[],"impulses":[],"devices":[],"source":[],"layers":[],"from_the_future":{"a":1}},"files":[]}"#;
        let doc = SyncDoc::parse(json).unwrap();
        assert_eq!(doc.profile.name, "Warm");
        assert_eq!(
            doc.unknown.get("from_the_future"),
            Some(&serde_json::json!({"a": 1}))
        );
        let again = SyncDoc::parse(&doc.json()).unwrap();
        assert_eq!(again, doc);
        assert!(doc.json().contains("from_the_future"));
    }

    /// A copy taken from the server with a field from a newer kōan is not
    /// an edit here, so it is never sent back without that field.
    #[test]
    fn a_newer_field_is_not_an_edit() {
        let _guard = lock();
        let server = Server::new();
        let (a, b) = (Device::new(), Device::new());
        a.on();
        Config::persist(|c| c.dsp.profiles.push(headphone())).unwrap();
        a.sync(&server);
        let (uid, json) = rows::live_docs(&server.conn, USER).unwrap().pop().unwrap();
        let mut newer: serde_json::Value = serde_json::from_str(&json).unwrap();
        newer["profile"]["from_the_future"] = serde_json::json!({"a": 1});
        server.save(&uid, now_ms() + 1, &newer.to_string()).unwrap();
        for device in [&a, &b, &a, &b] {
            let synced = device.sync(&server);
            assert_eq!(synced.sent, 0, "{synced:?}");
        }
        let (_, kept) = rows::live_docs(&server.conn, USER).unwrap().pop().unwrap();
        assert!(kept.contains("from_the_future"), "{kept}");
        assert_eq!(
            b.profile("HD 650 (AutoEQ, oratory1990)").unwrap().filters,
            vec![band(3.0)]
        );
    }

    /// What two profiles play, not how they are written down.
    #[test]
    fn profiles_that_would_sound_the_same_are_the_same() {
        let none = |_: &str| None;
        let peak = |hz: f64, db: f64| {
            DspFilter::Band(EqFilter {
                kind: EqFilterKind::Peaking,
                freq: hz,
                gain_db: db,
                q: 1.0,
                channels: vec![],
            })
        };
        let a = doc(vec![peak(100.0, 3.0), peak(1000.0, -2.0)], vec![]);
        let reordered = doc(vec![peak(1000.0, -2.0), peak(100.0, 3.0)], vec![]);
        assert!(sounds_same(&a, &reordered, &none), "bands commute");
        let close = doc(vec![peak(100.0, 3.04), peak(1000.0, -2.0)], vec![]);
        assert!(sounds_same(&a, &close, &none), "0.04 dB is nothing heard");
        let extra = doc(
            vec![peak(100.0, 3.0), peak(1000.0, -2.0), peak(5000.0, 1.0)],
            vec![],
        );
        assert!(!sounds_same(&a, &extra, &none), "one band more");
        let measured = |target: &str| {
            let mut d = a.clone();
            d.profile.measurement = Some(crate::config::DspMeasurement {
                ear: crate::config::DspEar::In,
                target: target.into(),
                fit: Default::default(),
            });
            d
        };
        assert!(
            !sounds_same(
                &measured("harman-in-ear-2019"),
                &measured("diffuse-field-iso-11904-1"),
                &none
            ),
            "one measurement corrected to two targets"
        );
        let tuned = |target: &str| {
            let mut d = a.clone();
            d.profile.tuned_for = Some(target.into());
            d
        };
        assert!(
            !sounds_same(
                &tuned("harman-over-ear-2018"),
                &tuned("diffuse-field-gras-kemar"),
                &none
            ),
            "one tuning made against two targets"
        );
        let mut grouped = a.clone();
        grouped.profile.group = true;
        assert!(
            !sounds_same(&a, &grouped, &none),
            "one at a time is not together"
        );
        let mut renamed = a.clone();
        renamed.profile.name = "Y".into();
        renamed.profile.origin = Some("iPhone".into());
        assert!(sounds_same(&a, &renamed, &none), "names and origins aside");

        // Around a delay, order counts.
        let delay = DspFilter::Delay(crate::config::Delay {
            ms: 1.0,
            ..Default::default()
        });
        let before = doc(
            vec![peak(100.0, 3.0), delay.clone(), peak(1000.0, -2.0)],
            vec![],
        );
        let after = doc(vec![peak(1000.0, -2.0), delay, peak(100.0, 3.0)], vec![]);
        assert!(!sounds_same(&before, &after, &none));

        // Impulse responses by content.
        let ir = |sha: char| {
            let mut d = doc(
                vec![],
                vec![SyncFile {
                    name: "48000.wav".into(),
                    sha256: sha.to_string().repeat(64),
                    size: 10,
                }],
            );
            d.profile.impulses = vec!["48000.wav".into()];
            d
        };
        assert!(sounds_same(&ir('a'), &ir('a'), &none));
        assert!(
            !sounds_same(&ir('a'), &ir('b'), &none),
            "a different response"
        );
    }

    #[test]
    fn a_doc_cannot_name_a_file_outside_its_folder() {
        let doc = |name: &str| SyncDoc {
            profile: DspProfile {
                name: "Room".into(),
                ..Default::default()
            },
            files: vec![SyncFile {
                name: name.into(),
                sha256: "a".repeat(64),
                size: 1,
            }],
            unknown: Default::default(),
        };
        for bad in ["../config.toml", ".hidden", "a/b", ""] {
            assert!(SyncDoc::parse(&doc(bad).json()).is_err(), "{bad:?}");
        }
        let mut huge = doc("x.wav");
        huge.files[0].size = MAX_FILE + 1;
        assert!(SyncDoc::parse(&huge.json()).is_err());
        assert!(SyncDoc::parse(&doc("x.wav").json()).is_ok());
    }

    /// A profile another device or the server sends out of bounds arrives
    /// within them, assigned to no output here, and plays.
    #[test]
    fn a_profile_out_of_bounds_arrives_within_them() {
        let _guard = lock();
        let server = Server::new();
        let b = Device::new();
        let hostile = r#"{"profile":{"name":"Loud","devices":["Built-in Output"],
            "filters":[{"type":"delay","ms":1e12},{"type":"peaking","freq":1000.0,"gain_db":200.0,"q":1.0}],
            "impulses":[]},"files":[]}"#;
        rows::save(
            &server.conn,
            USER,
            "0199b5a2-6c1e-7cc3-9d2a-3f5b1e0c4d21",
            1,
            Some(hostile),
        )
        .unwrap();
        b.sync(&server);
        let loud = b.profile("Loud").expect("applied");
        assert!(loud.devices.is_empty(), "assigned to nothing here");
        assert_eq!(
            loud.filters[0],
            DspFilter::Delay(crate::config::Delay {
                ms: 2000.0,
                ..Default::default()
            })
        );
        b.on();
        let cfg = Config::cached();
        let setup = crate::audio::dsp::Setup::load(&loud, &cfg.dsp.profiles, &config::config_dir());
        assert!(setup.is_ok(), "it plays");
    }

    /// Folders go by the slug of a name: a profile from elsewhere whose name
    /// maps to a folder of one kept here is named for its device, and the
    /// one here for this device, with its files: never overwritten or sent.
    #[test]
    fn a_folder_is_never_shared() {
        let _guard = lock();
        let server = Server::new();
        let (a, b) = (Device::named("Mac"), Device::named("iPhone"));
        b.on();
        let room = crate::audio::dsp::profiles::dir("Room");
        std::fs::create_dir_all(&room).unwrap();
        std::fs::write(room.join("48000.wav"), b"kept here").unwrap();
        Config::persist(|c| {
            c.dsp.profiles.push(DspProfile {
                name: "Room".into(),
                impulses: vec![Path::new("dsp/room/48000.wav").into()],
                scope: Some(DspScope::Device),
                ..Default::default()
            })
        })
        .unwrap();
        b.sync(&server);
        a.on();
        Config::persist(|c| {
            c.dsp.profiles.push(DspProfile {
                name: "ROOM".into(),
                filters: vec![band(1.0)],
                scope: Some(DspScope::Everywhere),
                ..Default::default()
            })
        })
        .unwrap();
        a.sync(&server);
        b.sync(&server);
        let renamed = b.profile("Room (iPhone)").expect("renamed, files and all");
        assert_eq!(renamed.scope, Some(DspScope::Device));
        b.on();
        assert_eq!(
            std::fs::read(config::config_dir().join(&renamed.impulses[0])).unwrap(),
            b"kept here"
        );
        assert!(b.profile("ROOM (Mac)").is_some());

        // Two profiles here whose names share a folder: neither is sent.
        Config::persist(|c| {
            for name in ["Desk", "desk!"] {
                c.dsp.profiles.push(DspProfile {
                    name: name.into(),
                    filters: vec![band(1.0)],
                    scope: Some(DspScope::Everywhere),
                    ..Default::default()
                });
            }
        })
        .unwrap();
        let cfg = Config::cached();
        let shared = cfg.dsp.profiles.iter().find(|p| p.name == "desk!").unwrap();
        assert!(doc_of(shared).unwrap_err().contains("shares its folder"));
    }
}
