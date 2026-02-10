use security_framework::passwords::{
    delete_generic_password, get_generic_password, set_generic_password,
};
use thiserror::Error;

const SERVICE_NAME: &str = "koan";

#[derive(Debug, Error)]
pub enum CredentialError {
    #[error("keychain error: {0}")]
    Keychain(#[from] security_framework::base::Error),
    #[error("password not found")]
    NotFound,
    #[error("invalid utf-8 in password")]
    InvalidUtf8,
}

/// Store a password in macOS Keychain.
/// `account` should be the server URL or identifier.
pub fn store_password(account: &str, password: &str) -> Result<(), CredentialError> {
    // Delete existing entry first (set_generic_password errors on duplicate).
    let _ = delete_generic_password(SERVICE_NAME, account);
    set_generic_password(SERVICE_NAME, account, password.as_bytes())?;
    Ok(())
}

/// Retrieve a password from macOS Keychain.
pub fn get_password(account: &str) -> Result<String, CredentialError> {
    let bytes = get_generic_password(SERVICE_NAME, account)?;
    String::from_utf8(bytes).map_err(|_| CredentialError::InvalidUtf8)
}

/// Delete a password from macOS Keychain.
pub fn delete_password(account: &str) -> Result<(), CredentialError> {
    delete_generic_password(SERVICE_NAME, account)?;
    Ok(())
}
