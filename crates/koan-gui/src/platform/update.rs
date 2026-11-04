//! Keeping kōan up to date (`updates.auto`).
//!
//! What an update can do depends on how this copy was installed. The DMG's
//! app on macOS and the AppImage on Linux replace themselves: the release's
//! file is fetched and checked against the digest GitHub published, and on
//! macOS the app inside it against the Developer ID this one is signed with.
//! The AppImage is renamed into place at once, since a running AppImage
//! keeps its own mounted image; the Mac's bundle is swapped by a script that
//! waits for this process to exit. The Homebrew cask and the `.deb` belong
//! to their package managers, and an app bundle kōan cannot write to belongs
//! to an administrator: those are only told a version is out. A build from
//! source never checks.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use koan_core::updates::{self, Release};
use koan_kit::app::*;

use super::about::VERSION;
use crate::theme::Tone;

/// How often a running kōan looks again, after the check at launch.
const EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// How often it asks whether `EVERY` has passed: by the wall clock, which
/// moves while the computer sleeps, as a timer's monotonic clock does not.
const TICK: Duration = Duration::from_secs(60 * 60);

/// How this copy can be updated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Install {
    /// Not worked out yet.
    Unknown,
    /// Run from source, or signed by no one: no checks.
    Source,
    /// The app bundle at this path, with this signing identifier and team,
    /// which kōan can write over.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Bundle {
        app: PathBuf,
        id: String,
        team: String,
    },
    /// The AppImage at this path.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    AppImage(PathBuf),
    /// Kept by something else, which installs the new version: the reason.
    Told(&'static str),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Idle,
    Checking,
    Current,
    Fetching(String),
    /// The version installed or staged, and what is left to do: on macOS,
    /// quitting.
    Ready(String),
    /// A newer version this install cannot take itself, and its page.
    Available {
        version: String,
        page: String,
    },
    Failed(String),
}

pub struct Updates {
    pub install: Install,
    pub state: State,
    /// The app bundle fetched and checked, swapped in when kōan quits.
    staged: Option<PathBuf>,
    /// Open kōan again once the update is in.
    relaunch: bool,
    auto: bool,
    /// When GitHub was last asked.
    last: Option<SystemTime>,
    /// The version last announced to an install that cannot take it.
    told: Option<String>,
    /// Cleared to drop the fetch running: what it fetched is not installed.
    wanted: Option<Arc<AtomicBool>>,
    check: Option<Task<()>>,
    _schedule: Option<Task<()>>,
}

impl Global for Updates {}

/// Works out how this copy was installed, then checks at launch and daily
/// while `updates.auto` is on.
pub fn init(engine: &std::sync::Arc<koan_runtime::KoanEngine>, cx: &mut App) {
    cx.set_global(Updates {
        install: Install::Unknown,
        state: State::Idle,
        staged: None,
        relaunch: false,
        auto: false,
        last: None,
        told: None,
        wanted: None,
        check: None,
        _schedule: None,
    });
    let engine = engine.clone();
    let schedule = cx.spawn(async move |cx| {
        let Some(install) = off_thread(install).await else {
            return;
        };
        let auto = engine.settings().await.updates_auto;
        log::info!("updates: {install:?}, automatic {auto}");
        let source = install == Install::Source;
        cx.update(|cx| {
            let updates = cx.global_mut::<Updates>();
            updates.install = install;
            updates.auto = auto;
            cx.refresh_windows();
        });
        if source {
            return;
        }
        loop {
            let due = cx.update(|cx| {
                let u = cx.global::<Updates>();
                u.auto
                    && u.last
                        // A clock set back since counts as due.
                        .is_none_or(|last| last.elapsed().map_or(true, |since| since >= EVERY))
            });
            if due {
                cx.update(check);
            }
            cx.background_executor().timer(TICK).await;
        }
    });
    cx.global_mut::<Updates>()._schedule = Some(schedule);
    cx.on_app_quit(|cx| {
        apply_on_quit(cx);
        async {}
    })
    .detach();
}

/// `updates.auto` as the file says now, written by Settings or by hand:
/// turned on, a check now; turned off, a fetch running is dropped and a
/// bundle waiting for quit is not installed.
pub fn follow(auto: bool, cx: &mut App) {
    if !cx.has_global::<Updates>() {
        return;
    }
    let updates = cx.global_mut::<Updates>();
    let was = std::mem::replace(&mut updates.auto, auto);
    if auto && !was {
        check(cx);
    } else if was && !auto {
        if let Some(wanted) = updates.wanted.take() {
            wanted.store(false, Ordering::Relaxed);
        }
        updates.check = None;
        let dropped = updates.staged.take().is_some();
        if dropped || matches!(updates.state, State::Checking | State::Fetching(_)) {
            set_state(State::Idle, cx);
        }
    }
}

pub fn install_kind(cx: &App) -> Install {
    cx.try_global::<Updates>()
        .map_or(Install::Unknown, |u| u.install.clone())
}

pub fn state(cx: &App) -> State {
    cx.try_global::<Updates>()
        .map_or(State::Idle, |u| u.state.clone())
}

/// Asks GitHub for a newer release, and fetches it where this install takes
/// it itself. Nothing while a check runs or an update waits.
pub fn check(cx: &mut App) {
    let Some(updates) = cx.try_global::<Updates>() else {
        return;
    };
    let install = updates.install.clone();
    if matches!(install, Install::Unknown | Install::Source)
        || matches!(
            updates.state,
            State::Checking | State::Fetching(_) | State::Ready(_)
        )
    {
        return;
    }
    let wanted = Arc::new(AtomicBool::new(true));
    let updates = cx.global_mut::<Updates>();
    updates.wanted = Some(wanted.clone());
    set_state(State::Checking, cx);
    let task = cx.spawn(async move |cx| {
        let Some(found) = off_thread(|| updates::newer_than(VERSION)).await else {
            return;
        };
        if found.is_ok() {
            // A failed check is tried again at the next tick, not tomorrow.
            cx.update(|cx| cx.global_mut::<Updates>().last = Some(SystemTime::now()));
        }
        let release = match found {
            Ok(Some(release)) => release,
            Ok(None) => {
                cx.update(|cx| set_state(State::Current, cx));
                return;
            }
            Err(e) => {
                log::warn!("updates: checking: {e}");
                cx.update(|cx| set_state(State::Failed(e.to_string()), cx));
                return;
            }
        };
        let version = release.version.clone();
        if let Install::Told(_) = install {
            cx.update(|cx| {
                let told = &mut cx.global_mut::<Updates>().told;
                if told.as_ref() != Some(&version) {
                    *told = Some(version.clone());
                    crate::notice::show(format!("kōan {version} is out"), Tone::Muted, cx);
                }
                set_state(
                    State::Available {
                        version,
                        page: release.page,
                    },
                    cx,
                );
            });
            return;
        }
        cx.update(|cx| set_state(State::Fetching(version.clone()), cx));
        let going = wanted.clone();
        let Some(staged) = off_thread(move || stage(&install, &release, &going)).await else {
            return;
        };
        if !wanted.load(Ordering::Relaxed) {
            return;
        }
        cx.update(|cx| match staged {
            Ok(staged) => {
                log::info!("updates: kōan {version} is ready");
                cx.global_mut::<Updates>().staged = staged;
                crate::notice::show(
                    format!("kōan {version} is ready: it opens next time kōan starts"),
                    Tone::Muted,
                    cx,
                );
                set_state(State::Ready(version), cx);
            }
            Err(e) => {
                log::warn!("updates: fetching {version}: {e}");
                set_state(State::Failed(e), cx);
            }
        });
    });
    cx.global_mut::<Updates>().check = Some(task);
}

/// Quits, and opens the new version once it is in.
pub fn restart(cx: &mut App) {
    if cx.has_global::<Updates>() {
        cx.global_mut::<Updates>().relaunch = true;
    }
    cx.quit();
}

fn set_state(state: State, cx: &mut App) {
    cx.global_mut::<Updates>().state = state;
    cx.refresh_windows();
}

/// Hands a staged bundle, or a relaunch, to a script that waits for this
/// process to exit: a bundle is not replaced under a running app, and a
/// second kōan started while this one runs would only hand it its files.
fn apply_on_quit(cx: &mut App) {
    let Some(updates) = cx.try_global::<Updates>() else {
        return;
    };
    let relaunch = if updates.relaunch { "1" } else { "0" };
    let pid = std::process::id().to_string();
    let args: Vec<String> = match (&updates.install, &updates.staged) {
        (Install::Bundle { app, .. }, Some(staged)) => vec![
            SWAP.into(),
            pid,
            app.display().to_string(),
            staged.display().to_string(),
            relaunch.into(),
        ],
        (Install::AppImage(path), _) if updates.relaunch => {
            vec![RELAUNCH.into(), pid, path.display().to_string()]
        }
        _ => return,
    };
    use std::os::unix::process::CommandExt as _;
    let spawned = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(&args[0])
        .arg("koan-update")
        .args(&args[1..])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        // Out of the terminal's process group, so the Ctrl-C that quit kōan
        // does not stop the swap.
        .process_group(0)
        .spawn();
    if let Err(e) = spawned {
        log::warn!("updates: could not start the swap: {e}");
    }
}

/// Moves the running bundle aside, copies the staged one into its place and
/// removes the old, or puts the old back if the copy failed.
const SWAP: &str = r#"
pid=$1 app=$2 new=$3 relaunch=$4
while kill -0 "$pid" 2>/dev/null; do sleep 0.2; done
old="$app.old-$$"
if mv "$app" "$old"; then
    if ditto "$new" "$app"; then
        rm -rf "$old" "$new"
    else
        rm -rf "$app"
        mv "$old" "$app"
    fi
fi
[ "$relaunch" = 1 ] && open "$app"
"#;

const RELAUNCH: &str = r#"
pid=$1 path=$2
while kill -0 "$pid" 2>/dev/null; do sleep 0.2; done
exec "$path"
"#;

/// Runs blocking work on a thread of its own: a release's download takes
/// minutes on a slow link, which GPUI's background executor is not for.
/// `None` if the thread could not start or panicked.
fn off_thread<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> impl Future<Output = Option<T>> {
    let (tx, rx) = futures::channel::oneshot::channel();
    let spawned = std::thread::Builder::new()
        .name("koan-update".into())
        .spawn(move || {
            let _ = tx.send(work());
        });
    if let Err(e) = &spawned {
        log::warn!("updates: could not start a thread: {e}");
    }
    async move { rx.await.ok() }
}

/// Fetches and checks the release's file for `install`. The AppImage is put
/// in place at once, unless `wanted` was cleared meanwhile; a bundle is
/// staged and its path returned.
fn stage(
    install: &Install,
    release: &Release,
    wanted: &AtomicBool,
) -> Result<Option<PathBuf>, String> {
    match install {
        Install::Bundle { id, team, .. } => stage_bundle(id, team, release).map(Some),
        Install::AppImage(path) => {
            let asset = release
                .asset(&format!("koan-gui-{}.AppImage", std::env::consts::ARCH))
                .map_err(|e| e.to_string())?;
            let new = path.with_extension("AppImage.new");
            updates::fetch(asset, &new).map_err(|e| e.to_string())?;
            use std::os::unix::fs::PermissionsExt as _;
            if !wanted.load(Ordering::Relaxed) {
                let _ = std::fs::remove_file(&new);
                return Ok(None);
            }
            std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755))
                .and_then(|()| std::fs::rename(&new, path))
                .map_err(|e| {
                    let _ = std::fs::remove_file(&new);
                    e.to_string()
                })?;
            Ok(None)
        }
        _ => Ok(None),
    }
}

/// The DMG fetched, its app copied out and checked: signed by `team`, with
/// this app's identifier `id`.
fn stage_bundle(id: &str, team: &str, release: &Release) -> Result<PathBuf, String> {
    let dir = koan_core::places::cache().join("update");
    let mount = dir.join("mount");
    // An image left attached by a run that was killed while staging.
    if mount.exists() {
        let _ = run(
            "hdiutil",
            &["detach".as_ref(), "-force".as_ref(), mount.as_os_str()],
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let dmg = dir.join("Koan.dmg");
    let asset = release.asset("Koan.dmg").map_err(|e| e.to_string())?;
    updates::fetch(asset, &dmg).map_err(|e| e.to_string())?;
    run(
        "hdiutil",
        &[
            "attach".as_ref(),
            "-nobrowse".as_ref(),
            "-readonly".as_ref(),
            "-noautoopen".as_ref(),
            "-mountpoint".as_ref(),
            mount.as_os_str(),
            dmg.as_os_str(),
        ],
    )?;
    let copied = (|| {
        let app = std::fs::read_dir(&mount)
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|e| e == "app"))
            .ok_or("the disk image holds no app")?;
        let staged = dir.join(app.file_name().ok_or("the app has no name")?);
        run("ditto", &[app.as_os_str(), staged.as_os_str()])?;
        Ok::<_, String>(staged)
    })();
    let _ = run(
        "hdiutil",
        &["detach".as_ref(), "-force".as_ref(), mount.as_os_str()],
    );
    let _ = std::fs::remove_file(&dmg);
    let staged = copied?;
    let requirement = format!(
        "=anchor apple generic and identifier \"{id}\" and certificate leaf[subject.OU] = \"{team}\""
    );
    run(
        "codesign",
        &[
            "--verify".as_ref(),
            "--deep".as_ref(),
            "--strict".as_ref(),
            "-R".as_ref(),
            requirement.as_ref(),
            staged.as_os_str(),
        ],
    )
    .map_err(|e| {
        let _ = std::fs::remove_dir_all(&staged);
        format!("the new version's signature does not match this one's: {e}")
    })?;
    Ok(staged)
}

fn run(program: &str, args: &[&std::ffi::OsStr]) -> Result<String, String> {
    let out = std::process::Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("{program}: {e}"))?;
    let text = |b: &[u8]| String::from_utf8_lossy(b).trim().to_owned();
    if out.status.success() {
        Ok(text(&out.stdout))
    } else {
        Err(format!("{program}: {}", text(&out.stderr)))
    }
}

/// The `.app` an executable inside `Contents/MacOS` belongs to.
#[cfg(any(target_os = "macos", test))]
fn bundle_of(exe: &Path) -> Option<PathBuf> {
    let app = exe.parent()?.parent()?.parent()?;
    app.extension()
        .is_some_and(|e| e == "app")
        .then(|| app.to_path_buf())
}

/// A bundle's signing identifier and team, when it has a team: a build signed
/// ad hoc or with the self-signed development certificate has none.
#[cfg(target_os = "macos")]
fn signature(app: &Path) -> Option<(String, String)> {
    let out = std::process::Command::new("codesign")
        .args(["-dv", "--verbose=2"])
        .arg(app)
        .output()
        .ok()?;
    // codesign describes a signature on stderr.
    let text = String::from_utf8_lossy(&out.stderr);
    let field = |name: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(name)?.strip_prefix('='))
            .map(str::to_owned)
    };
    let team = field("TeamIdentifier").filter(|t| t != "not set")?;
    Some((field("Identifier")?, team))
}

/// Whether a file can be made in `dir`: an app kōan can replace itself.
fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".koan-update-{}", std::process::id()));
    let made = std::fs::File::create(&probe).is_ok();
    let _ = std::fs::remove_file(&probe);
    made
}

#[cfg(target_os = "macos")]
fn install() -> Install {
    let Some(app) = std::env::current_exe().ok().and_then(|e| bundle_of(&e)) else {
        return Install::Source;
    };
    let Some((id, team)) = signature(&app) else {
        return Install::Source;
    };
    let cask = ["/opt/homebrew", "/usr/local"]
        .iter()
        .any(|prefix| Path::new(prefix).join("Caskroom/koan-app").exists());
    // The cask installs into /Applications; a copy elsewhere is the DMG's.
    if cask && app.starts_with("/Applications") {
        return Install::Told("Homebrew keeps this copy up to date: brew upgrade --cask koan-app.");
    }
    if !app.parent().is_some_and(writable) {
        return Install::Told(
            "kōan cannot write to the folder it is in, so it cannot replace itself.",
        );
    }
    Install::Bundle { app, id, team }
}

#[cfg(not(target_os = "macos"))]
fn install() -> Install {
    // An AppImage's runtime sets both for everything started under it, so a
    // kōan started from another AppImage's terminal inherits them: only a
    // kōan running from inside the image is the AppImage.
    let exe = std::env::current_exe().ok();
    let inside = std::env::var_os("APPDIR")
        .filter(|dir| !dir.is_empty())
        .is_some_and(|dir| exe.as_ref().is_some_and(|exe| exe.starts_with(&dir)));
    if let Some(path) = std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .filter(|_| inside)
    {
        if path.parent().is_some_and(writable) {
            return Install::AppImage(path);
        }
        return Install::Told(
            "kōan cannot write to the folder its AppImage is in, so it cannot replace itself.",
        );
    }
    let packaged = exe.is_some_and(|exe| exe.starts_with("/usr"));
    if packaged {
        return Install::Told(
            "The package manager installs new versions: download the release's .deb.",
        );
    }
    Install::Source
}

#[cfg(test)]
mod tests {
    use super::bundle_of;
    use std::path::Path;

    #[test]
    fn finds_the_bundle_an_executable_is_in() {
        assert_eq!(
            bundle_of(Path::new("/Applications/kōan.app/Contents/MacOS/koan-gui")).as_deref(),
            Some(Path::new("/Applications/kōan.app"))
        );
        assert_eq!(
            bundle_of(Path::new("/Users/me/koan/target/release/koan-gui")),
            None
        );
    }
}
