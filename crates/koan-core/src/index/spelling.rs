//! Paths as the filesystem spells them.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};

use unicode_normalization::UnicodeNormalization;

/// Resolves a path handed in from outside to the spelling the filesystem uses.
///
/// A drop from Finder, a folder typed into settings: each arrives in whatever
/// form its source favours, and Foundation favours precomposed accents where a
/// Mac-written disk holds them decomposed. Both open the file — APFS and HFS+
/// don't tell the two apart — but `tracks.path` does, and a scan stores the
/// directory entry's own bytes. A file indexed under the other spelling is a
/// file the next scan has no row for, so it gets a second one.
///
/// Case is the same story: a case-insensitive volume opens `The Beatles` as
/// `The beatles`, and a scan stores whichever the directory holds.
///
/// Each component is looked up in the directory that holds it and replaced by
/// the entry that names it: exact bytes first, then the same name in another
/// Unicode normalisation, then, only where the filesystem itself opens the
/// name as given, the same name in another case. Symlinks aren't
/// followed, so a symlinked library root keeps the path it was configured as.
/// A directory that can't be read leaves its component as given. Listings are
/// kept for the life of the resolver: a drop of a hundred files from one folder
/// reads that folder once.
#[derive(Default)]
pub struct Spelling {
    listings: HashMap<PathBuf, Vec<OsString>>,
}

impl Spelling {
    pub fn on_disk(&mut self, path: &Path) -> PathBuf {
        let mut out = PathBuf::new();
        for component in path.components() {
            let Component::Normal(name) = component else {
                out.push(component.as_os_str());
                continue;
            };
            let dir = if out.as_os_str().is_empty() {
                PathBuf::from(".")
            } else {
                out.clone()
            };
            match self.entry(&dir, name) {
                Some(spelled) => out.push(spelled),
                None => out.push(name),
            }
        }
        out
    }

    fn entry(&mut self, dir: &Path, name: &OsStr) -> Option<OsString> {
        if self.listing(dir).iter().any(|n| n == name) {
            return Some(name.to_os_string());
        }
        // Where the filesystem does not open the name as given, another
        // spelling of it is another file, and this one does not exist.
        if !dir.join(name).exists() {
            return None;
        }
        // It opens but is not in the listing as given: another spelling of
        // an entry, or an entry made since the listing was read. Read again,
        // so the second is found exactly rather than matched to a sibling.
        self.listings.remove(dir);
        find_spelling(self.listing(dir), name)
    }

    fn listing(&mut self, dir: &Path) -> &[OsString] {
        self.listings.entry(dir.to_path_buf()).or_insert_with(|| {
            std::fs::read_dir(dir)
                .map(|entries| entries.flatten().map(|e| e.file_name()).collect())
                .unwrap_or_default()
        })
    }
}

/// The entry naming `name` in another Unicode normalisation, else in another
/// case.
fn find_spelling(listing: &[OsString], name: &OsStr) -> Option<OsString> {
    if let Some(exact) = listing.iter().find(|n| *n == name) {
        return Some(exact.clone());
    }
    let wanted: String = name.to_string_lossy().nfc().collect();
    let folded = |n: &OsString| n.to_string_lossy().nfc().collect::<String>();
    listing
        .iter()
        .find(|n| folded(n) == wanted)
        .or_else(|| {
            let wanted = wanted.to_lowercase();
            listing.iter().find(|n| folded(n).to_lowercase() == wanted)
        })
        .cloned()
}

/// One path, resolved once.
pub fn on_disk(path: &Path) -> PathBuf {
    Spelling::default().on_disk(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spellings() -> (String, String) {
        let nfd: String = "Flügel".nfd().collect();
        let nfc: String = "Flügel".nfc().collect();
        assert_ne!(nfd, nfc);
        (nfd, nfc)
    }

    #[test]
    fn a_precomposed_spelling_becomes_the_directorys_own() {
        let dir = tempfile::tempdir().unwrap();
        let (nfd, nfc) = spellings();
        std::fs::create_dir(dir.path().join(&nfd)).unwrap();
        std::fs::write(dir.path().join(&nfd).join("song.wav"), b"").unwrap();

        let asked = dir.path().join(&nfc).join("song.wav");
        let got = on_disk(&asked);
        // Where the filesystem tells the two apart, the other is another file.
        let disk = if dir.path().join(&nfc).exists() {
            dir.path().join(&nfd).join("song.wav")
        } else {
            asked
        };
        assert_eq!(
            got.as_os_str().as_encoded_bytes(),
            disk.as_os_str().as_encoded_bytes(),
            "the bytes the directory holds, not the ones asked with"
        );
    }

    #[test]
    fn the_disks_own_spelling_passes_through() {
        let dir = tempfile::tempdir().unwrap();
        let (nfd, _) = spellings();
        std::fs::create_dir(dir.path().join(&nfd)).unwrap();
        let disk = dir.path().join(&nfd);
        assert_eq!(on_disk(&disk), disk);
    }

    #[test]
    fn a_name_in_another_case_becomes_the_directorys_own_only_where_it_opens() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("The beatles")).unwrap();
        let asked = dir.path().join("The Beatles").join("song.wav");
        let insensitive = dir.path().join("THE BEATLES").exists();
        let expected = if insensitive {
            dir.path().join("The beatles").join("song.wav")
        } else {
            asked.clone()
        };
        assert_eq!(on_disk(&asked), expected);
    }

    #[test]
    fn an_entry_made_after_its_directory_was_listed_is_found_as_itself() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("b")).unwrap();
        let mut spelling = Spelling::default();
        assert_eq!(
            spelling.on_disk(&dir.path().join("b")),
            dir.path().join("b")
        );

        // On a case-insensitive volume this is `b` again.
        std::fs::create_dir_all(dir.path().join("B")).unwrap();
        let expected = if std::fs::read_dir(dir.path()).unwrap().count() == 2 {
            dir.path().join("B")
        } else {
            dir.path().join("b")
        };
        assert_eq!(spelling.on_disk(&dir.path().join("B")), expected);
    }

    #[test]
    fn an_unreadable_component_is_left_as_given() {
        let (_, nfc) = spellings();
        let asked = Path::new("/nowhere/at/all").join(&nfc).join("song.wav");
        assert_eq!(on_disk(&asked), asked);
    }
}
