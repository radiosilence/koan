//! Newer releases of kōan: which one GitHub calls latest, whether it is newer
//! than this build, and fetching one of its files with its digest checked.
//! Installing what was fetched is the app's, since it depends on how the app
//! was installed.

use std::io::{Read, Write};
use std::path::Path;

use serde::Deserialize;
use sha2::{Digest, Sha256};

const LATEST: &str = "https://api.github.com/repos/radiosilence/koan/releases/latest";
const USER_AGENT: &str = concat!("koan/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("could not reach GitHub: {0}")]
    Http(#[from] reqwest::Error),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("the release has no {0}")]
    MissingAsset(String),
    #[error("the download does not match its published digest")]
    Digest,
}

/// A published release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// `0.71.0`, without the tag's `v`.
    pub version: String,
    /// The release's page, for installs that cannot update themselves.
    pub page: String,
    pub assets: Vec<Asset>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Asset {
    pub name: String,
    #[serde(rename = "browser_download_url")]
    pub url: String,
    /// `sha256:<hex>`, as GitHub computes it on upload.
    #[serde(default)]
    pub digest: Option<String>,
}

#[derive(Deserialize)]
struct Latest {
    tag_name: String,
    html_url: String,
    assets: Vec<Asset>,
}

impl Release {
    pub fn asset(&self, name: &str) -> Result<&Asset, UpdateError> {
        self.assets
            .iter()
            .find(|a| a.name == name)
            .ok_or_else(|| UpdateError::MissingAsset(name.into()))
    }
}

/// The latest release, when it is newer than `current`.
pub fn newer_than(current: &str) -> Result<Option<Release>, UpdateError> {
    let latest: Latest = client()?
        .get(LATEST)
        .header("Accept", "application/vnd.github+json")
        .send()?
        .error_for_status()?
        .json()?;
    let version = latest.tag_name.trim_start_matches('v').to_owned();
    Ok(is_newer(&version, current).then_some(Release {
        version,
        page: latest.html_url,
        assets: latest.assets,
    }))
}

/// Whether `candidate` is a later version than `current`. A version that
/// does not parse is never newer.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse(candidate), parse(current)) {
        (Some(c), Some(now)) => c > now,
        _ => false,
    }
}

fn parse(version: &str) -> Option<(u64, u64, u64)> {
    let core = version.split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse().ok());
    let v = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(v)
}

/// Downloads `asset` to `dest`, through `dest` with `.part` appended, and
/// renames it into place only once its digest matches the one GitHub
/// published. An asset published without one is refused.
pub fn fetch(asset: &Asset, dest: &Path) -> Result<(), UpdateError> {
    let expected = asset
        .digest
        .as_deref()
        .and_then(|d| d.strip_prefix("sha256:"))
        .ok_or(UpdateError::Digest)?
        .to_ascii_lowercase();
    let part = dest.with_extension(
        dest.extension()
            .map_or("part".into(), |e| format!("{}.part", e.to_string_lossy())),
    );
    let mut response = client()?.get(&asset.url).send()?.error_for_status()?;
    let mut file = std::fs::File::create(&part)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0; 1 << 16];
    loop {
        let n = response.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n])?;
    }
    file.sync_all()?;
    drop(file);
    let got: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if got != expected {
        let _ = std::fs::remove_file(&part);
        return Err(UpdateError::Digest);
    }
    std::fs::rename(&part, dest)?;
    Ok(())
}

fn client() -> Result<reqwest::blocking::Client, reqwest::Error> {
    reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        // A release's files run to a hundred megabytes; the timeout is for a
        // stalled connection, not a slow one.
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(30 * 60))
        .build()
}

#[cfg(test)]
mod tests {
    use super::is_newer;

    #[test]
    fn compares_versions_as_numbers() {
        assert!(is_newer("0.71.0", "0.70.3"));
        assert!(is_newer("0.70.10", "0.70.9"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("0.70.3", "0.70.3"));
        assert!(!is_newer("0.70.2", "0.70.3"));
        assert!(!is_newer("nonsense", "0.70.3"));
        assert!(!is_newer("0.71", "0.70.3"));
        assert!(is_newer("0.71.0", "0.70.3-dev"));
    }
}
