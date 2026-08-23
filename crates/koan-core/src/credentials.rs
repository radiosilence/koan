use std::collections::HashMap;

use keyring::Entry;
use thiserror::Error;

const SERVICE_NAME: &str = "koan";

#[derive(Debug, Error)]
pub enum CredentialError {
    #[error("keyring error: {0}")]
    Keyring(#[from] keyring::Error),
    #[error("password not found")]
    NotFound,
}

/// Store a password in the platform credential store.
/// macOS: Keychain. Linux: secret-service (GNOME Keyring / KDE Wallet).
/// `account` should be the server URL or identifier.
pub fn store_password(account: &str, password: &str) -> Result<(), CredentialError> {
    if let Ok(mut cache) = CACHE.write() {
        cache.insert(account.to_string(), Some(password.to_string()));
    }
    if keychain_disabled() {
        return Ok(());
    }
    let entry = Entry::new(SERVICE_NAME, account)?;
    entry.set_password(password)?;
    Ok(())
}

/// Whether this process may touch the credential store at all.
///
/// A keychain item's ACL is keyed on the reading binary's code signature, and a
/// `cargo test` binary is unsigned with a fresh hash every compile — so it can
/// never match, and "Always Allow" grants access to a binary that will not exist
/// after the next build. The prompt therefore returns on every single run.
///
/// `KOAN_NO_KEYCHAIN=1` opts out. `just check` sets it, so the test suite never
/// asks; set it yourself if you run `cargo test` directly.
fn keychain_disabled() -> bool {
    std::env::var_os("KOAN_NO_KEYCHAIN").is_some_and(|v| v != "0")
}

/// Answers already given, so the store is asked once per account per process.
///
/// Every `subsonic_client` builds its credentials from scratch, and the download
/// queue, radio, sync and sharing all build one — so a single session asked the
/// keychain repeatedly. Each ask is a chance for macOS to put up its "wants to
/// use your confidential information" dialog, and being asked five times for one
/// password is indistinguishable from the app being broken.
///
/// A miss is remembered too: an account with no password should be asked about
/// once, not on every attempt to reach a server that is not configured.
static CACHE: std::sync::LazyLock<std::sync::RwLock<HashMap<String, Option<String>>>> =
    std::sync::LazyLock::new(|| std::sync::RwLock::new(HashMap::new()));

/// Retrieve a password from the platform credential store.
pub fn get_password(account: &str) -> Result<String, CredentialError> {
    // Nothing to look up, and asking would prompt for a keychain koan has no
    // business opening.
    if account.is_empty() || keychain_disabled() {
        return Err(CredentialError::NotFound);
    }

    if let Ok(cache) = CACHE.read()
        && let Some(cached) = cache.get(account)
    {
        return cached.clone().ok_or(CredentialError::NotFound);
    }

    let entry = Entry::new(SERVICE_NAME, account)?;
    let result = match entry.get_password() {
        Ok(pw) => Ok(pw),
        Err(keyring::Error::NoEntry) => Err(CredentialError::NotFound),
        Err(e) => return Err(CredentialError::Keyring(e)),
    };

    // Only a definite answer is cached. A transient failure — the user dismissed
    // the dialog, the keychain was locked — must not be remembered as "no
    // password" for the rest of the session.
    if let Ok(mut cache) = CACHE.write() {
        cache.insert(account.to_string(), result.as_ref().ok().cloned());
    }
    result
}

/// Delete a password from the platform credential store.
pub fn delete_password(account: &str) -> Result<(), CredentialError> {
    if let Ok(mut cache) = CACHE.write() {
        cache.remove(account);
    }
    if keychain_disabled() {
        return Ok(());
    }
    let entry = Entry::new(SERVICE_NAME, account)?;
    entry.delete_credential()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `KOAN_NO_KEYCHAIN` has to be honoured, or the test suite prompts for the
    /// login password on every run — a cargo test binary is unsigned and gets a
    /// fresh hash each compile, so no keychain ACL can ever match it.
    #[test]
    fn the_opt_out_is_honoured() {
        // Safety: single-threaded within this test, and the value is restored.
        let previous = std::env::var_os("KOAN_NO_KEYCHAIN");
        unsafe { std::env::set_var("KOAN_NO_KEYCHAIN", "1") };
        assert!(keychain_disabled());
        assert!(matches!(
            get_password("https://example.invalid"),
            Err(CredentialError::NotFound)
        ));
        unsafe {
            match previous {
                Some(v) => std::env::set_var("KOAN_NO_KEYCHAIN", v),
                None => std::env::remove_var("KOAN_NO_KEYCHAIN"),
            }
        }
    }

    /// An empty account is not a lookup. Asking the keychain to find out would
    /// put a dialog in front of someone who has not configured a server.
    #[test]
    fn an_empty_account_never_reaches_the_store() {
        assert!(matches!(get_password(""), Err(CredentialError::NotFound)));
    }
}
