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
    let entry = Entry::new(SERVICE_NAME, account)?;
    entry.set_password(password)?;
    Ok(())
}

/// Retrieve a password from the platform credential store.
pub fn get_password(account: &str) -> Result<String, CredentialError> {
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
