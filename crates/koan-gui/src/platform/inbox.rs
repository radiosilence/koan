//! Files other apps gave kōan on iOS: what the share extension left in the
//! app group, and the copies iOS makes in `Documents/Inbox` for "Open in".

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

/// What a taken share's folder in the temporary directory is called.
const TAKEN_PREFIX: &str = "shared-";

/// Shares taken so far by this process.
static TAKEN: AtomicU64 = AtomicU64::new(0);

fn own_prefix() -> String {
    format!("{TAKEN_PREFIX}{}-", std::process::id())
}

/// What the share extension left in `inbox`, one folder per share: each moved
/// into `temp` first, so it is taken exactly once, and an import that stops
/// to ask about it still has its files.
pub fn take_shares(inbox: &Path, temp: &Path) -> Option<Vec<Vec<PathBuf>>> {
    let mut taken = Vec::new();
    for share in std::fs::read_dir(inbox).ok()?.flatten() {
        let n = TAKEN.fetch_add(1, Ordering::Relaxed);
        let to = temp.join(format!("{}{n}", own_prefix()));
        if std::fs::rename(share.path(), &to).is_err() {
            continue;
        }
        let mut files: Vec<PathBuf> = std::fs::read_dir(&to)
            .into_iter()
            .flatten()
            .flatten()
            .map(|file| file.path())
            .collect();
        files.sort();
        if files.is_empty() {
            let _ = std::fs::remove_dir_all(&to);
        } else {
            taken.push(files);
        }
    }
    Some(taken)
}

/// Deletes the shares earlier runs took. An import can wait on a question
/// for as long as the app lives, so this run's are left alone.
pub fn sweep_taken(temp: &Path) {
    let own = own_prefix();
    for entry in std::fs::read_dir(temp).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(TAKEN_PREFIX) && !name.starts_with(&own) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Deletes the copies iOS made in `inbox` before `before`, for imports that
/// never finished. The file whose opening launched the app is newer.
pub fn sweep_opened(inbox: &Path, before: SystemTime) {
    for entry in std::fs::read_dir(inbox).into_iter().flatten().flatten() {
        let made = entry
            .metadata()
            .and_then(|m| m.created())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        if made < before {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::Duration;

    fn share(inbox: &Path, name: &str, file: &str) {
        fs::create_dir_all(inbox.join(name)).unwrap();
        fs::write(inbox.join(name).join(file), "preset").unwrap();
    }

    #[test]
    fn a_second_share_in_one_run_is_taken_too() {
        let (inbox, temp) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        share(inbox.path(), "a", "one.txt");
        let first = take_shares(inbox.path(), temp.path()).unwrap();
        assert_eq!(first.len(), 1);
        assert!(first[0][0].exists(), "kept for an import that waits");

        share(inbox.path(), "b", "two.txt");
        let second = take_shares(inbox.path(), temp.path()).unwrap();
        assert_eq!(second.len(), 1);
        assert!(second[0][0].ends_with("two.txt"));
        assert_eq!(fs::read_dir(inbox.path()).unwrap().count(), 0);
    }

    #[test]
    fn an_empty_share_is_dropped() {
        let (inbox, temp) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        fs::create_dir_all(inbox.path().join("empty")).unwrap();
        assert_eq!(take_shares(inbox.path(), temp.path()).unwrap().len(), 0);
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
    }

    #[test]
    fn no_inbox_is_nothing_to_take() {
        let temp = tempfile::tempdir().unwrap();
        assert!(take_shares(&temp.path().join("absent"), temp.path()).is_none());
    }

    #[test]
    fn an_earlier_runs_shares_are_swept_and_this_ones_kept() {
        let temp = tempfile::tempdir().unwrap();
        share(temp.path(), "shared-1-0", "old.txt");
        let own = format!("{}0", own_prefix());
        share(temp.path(), &own, "waiting.txt");
        fs::create_dir_all(temp.path().join("unrelated")).unwrap();
        sweep_taken(temp.path());
        assert!(!temp.path().join("shared-1-0").exists());
        assert!(temp.path().join(own).exists());
        assert!(temp.path().join("unrelated").exists());
    }

    #[test]
    fn only_opened_copies_older_than_the_mark_go() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("eq.txt");
        fs::write(&file, "preset").unwrap();
        sweep_opened(dir.path(), SystemTime::now() - Duration::from_secs(600));
        assert!(file.exists(), "made since the mark");
        sweep_opened(dir.path(), SystemTime::now() + Duration::from_secs(600));
        assert!(!file.exists());
    }
}
