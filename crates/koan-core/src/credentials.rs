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

/// Retrieve a password from the platform credential store.
pub fn get_password(account: &str) -> Result<String, CredentialError> {
    // Nothing to look up, and asking would prompt for a keychain koan has no
    // business opening.
    if account.is_empty() || keychain_disabled() {
        return Err(CredentialError::NotFound);
    }
    let entry = Entry::new(SERVICE_NAME, account)?;
    match entry.get_password() {
        Ok(pw) => Ok(pw),
        Err(keyring::Error::NoEntry) => Err(CredentialError::NotFound),
        Err(e) => Err(CredentialError::Keyring(e)),
    }
}

/// Delete a password from the platform credential store.
pub fn delete_password(account: &str) -> Result<(), CredentialError> {
    let entry = Entry::new(SERVICE_NAME, account)?;
    entry.delete_credential()?;
    Ok(())
}
