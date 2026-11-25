//! The rules about accounts: what a username may be, who becomes the first
//! admin, and who may not be removed or demoted.
//!
//! The server never keeps a password it can read back. An account made by an
//! admin gets a generated password, shown once, and stored only as a hash.

use rusqlite::Connection;

use crate::users::{self, UserRow};
use crate::{ANONYMOUS, Role};

pub const MAX_USERNAME: usize = 64;
pub const MIN_PASSWORD: usize = 8;

#[derive(Debug, thiserror::Error)]
pub enum AccountError {
    #[error("usernames are 1 to {MAX_USERNAME} characters, without spaces")]
    BadUsername,
    #[error("passwords are at least {MIN_PASSWORD} characters")]
    ShortPassword,
    #[error("there is already an account called {0}")]
    Taken(String),
    #[error("{0} is reserved")]
    Reserved(String),
    #[error("there is no account called {0}")]
    NoSuchUser(String),
    #[error("this invite is not valid here, or has expired")]
    BadInvite,
    #[error("the last admin cannot be removed or demoted")]
    LastAdmin,
    #[error("this server already has accounts")]
    AlreadySetUp,
    #[error(transparent)]
    Other(#[from] Box<dyn std::error::Error + Send + Sync>),
}

pub(crate) fn other(e: impl std::error::Error + Send + Sync + 'static) -> AccountError {
    AccountError::Other(Box::new(e))
}

impl From<rusqlite::Error> for AccountError {
    fn from(e: rusqlite::Error) -> Self {
        other(e)
    }
}

/// An account just made: its id, for a token, and its generated password, for
/// the one time it is shown. The password is not kept anywhere it can be read
/// back.
#[derive(Debug)]
pub struct NewAccount {
    pub id: i64,
    pub password: String,
}

/// `username` trimmed, or why it cannot name an account.
pub fn valid_username(username: &str) -> Result<&str, AccountError> {
    let username = username.trim();
    if username.is_empty()
        || username.chars().count() > MAX_USERNAME
        || username.chars().any(char::is_whitespace)
    {
        return Err(AccountError::BadUsername);
    }
    if username.eq_ignore_ascii_case(ANONYMOUS) {
        return Err(AccountError::Reserved(username.to_owned()));
    }
    Ok(username)
}

/// Whether `password` is long enough to be one.
pub fn valid_password(password: &str) -> Result<&str, AccountError> {
    if password.chars().count() < MIN_PASSWORD {
        return Err(AccountError::ShortPassword);
    }
    Ok(password)
}

/// Create an account with a generated password.
pub fn create_account(
    conn: &Connection,
    username: &str,
    role: Role,
) -> Result<NewAccount, AccountError> {
    let username = valid_username(username)?;
    if users::get_user_by_username(conn, username)
        .map_err(other)?
        .is_some()
    {
        return Err(AccountError::Taken(username.to_owned()));
    }
    let password = crate::secret::generate_password().map_err(other)?;
    let id = users::create_user(conn, username, &password, role).map_err(other)?;
    Ok(NewAccount { id, password })
}

/// Make the first admin of a server that has no accounts, with the password
/// they chose. Refused once any account exists, and the check and the insert
/// are one transaction (a savepoint in the caller's, if it is in one), so two
/// people racing for an empty server cannot both become its admin.
pub fn create_first_admin(
    conn: &Connection,
    username: &str,
    password: &str,
) -> Result<i64, AccountError> {
    let username = valid_username(username)?;
    valid_password(password)?;
    crate::sql::atomically(conn, || {
        if users::has_users(conn).map_err(other)? {
            return Err(AccountError::AlreadySetUp);
        }
        users::create_user(conn, username, password, Role::Admin).map_err(other)
    })
}

/// The account called `username`, or `NoSuchUser`.
pub fn account(conn: &Connection, username: &str) -> Result<UserRow, AccountError> {
    users::get_user_by_username(conn, username)
        .map_err(other)?
        .ok_or_else(|| AccountError::NoSuchUser(username.to_owned()))
}

/// Give an account `password`, or a generated one when `None`, which is
/// returned. Ends every session, API key and app password the account has
/// (see [`users::update_password`]).
pub fn set_password(
    conn: &Connection,
    username: &str,
    password: Option<&str>,
) -> Result<String, AccountError> {
    account(conn, username)?;
    let password = match password {
        Some(p) => valid_password(p)?.to_owned(),
        None => crate::secret::generate_password().map_err(other)?,
    };
    users::update_password(conn, username, &password)
        .map_err(|e| AccountError::Other(e.to_string().into()))?;
    Ok(password)
}

/// Change an account's role, refusing to demote the last admin.
pub fn set_role(conn: &Connection, username: &str, role: Role) -> Result<(), AccountError> {
    let user = account(conn, username)?;
    if user.role == Role::Admin
        && role != Role::Admin
        && users::admin_count(conn).map_err(other)? <= 1
    {
        return Err(AccountError::LastAdmin);
    }
    users::update_role(conn, username, role).map_err(other)?;
    Ok(())
}

/// Delete an account, refusing to delete the last admin.
pub fn delete_account(conn: &Connection, username: &str) -> Result<(), AccountError> {
    let user = account(conn, username)?;
    if user.role == Role::Admin && users::admin_count(conn).map_err(other)? <= 1 {
        return Err(AccountError::LastAdmin);
    }
    users::delete_user(conn, user.id).map_err(other)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::password::verify_password;
    use crate::schema::test_db;

    #[test]
    fn the_first_admin_is_made_once() {
        let conn = test_db();
        assert!(matches!(
            create_first_admin(&conn, "ada", "short"),
            Err(AccountError::ShortPassword)
        ));
        assert!(matches!(
            create_first_admin(&conn, "a da", "long enough"),
            Err(AccountError::BadUsername)
        ));
        let id = create_first_admin(&conn, " ada ", "long enough").unwrap();
        let ada = users::get_user_by_id(&conn, id).unwrap().unwrap();
        assert_eq!((ada.username.as_str(), ada.role), ("ada", Role::Admin));
        assert!(matches!(
            create_first_admin(&conn, "mallory", "long enough"),
            Err(AccountError::AlreadySetUp)
        ));
        assert!(
            users::get_user_by_username(&conn, "mallory")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn the_first_admin_can_be_made_inside_a_callers_transaction() {
        let conn = test_db();
        conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        create_first_admin(&conn, "ada", "long enough").unwrap();
        assert!(matches!(
            create_first_admin(&conn, "bob", "long enough"),
            Err(AccountError::AlreadySetUp)
        ));
        conn.execute_batch("COMMIT").unwrap();
        assert_eq!(users::list_users(&conn).unwrap().len(), 1);
    }

    #[test]
    fn accounts_are_created_and_guarded() {
        let conn = test_db();
        create_account(&conn, "owner", Role::Admin).unwrap();
        assert!(matches!(
            create_account(&conn, "owner", Role::User),
            Err(AccountError::Taken(_))
        ));
        assert!(matches!(
            create_account(&conn, "two words", Role::User),
            Err(AccountError::BadUsername)
        ));
        assert!(matches!(
            create_account(&conn, "anonymous", Role::User),
            Err(AccountError::Reserved(_))
        ));
        assert!(matches!(
            set_role(&conn, "owner", Role::User),
            Err(AccountError::LastAdmin)
        ));
        assert!(matches!(
            delete_account(&conn, "owner"),
            Err(AccountError::LastAdmin)
        ));
        assert!(matches!(
            set_password(&conn, "nobody", None),
            Err(AccountError::NoSuchUser(_))
        ));
        create_account(&conn, "second", Role::Admin).unwrap();
        set_role(&conn, "owner", Role::User).unwrap();
        delete_account(&conn, "owner").unwrap();
    }

    #[test]
    fn a_new_password_signs_every_device_out() {
        let conn = test_db();
        let made = create_account(&conn, "sarita", Role::Readonly).unwrap();
        let hash = |conn| account(conn, "sarita").unwrap().password_hash;
        verify_password(&made.password, &hash(&conn)).unwrap();
        let (_, key) = crate::api_keys::create_api_key(&conn, made.id, "phone").unwrap();

        assert!(matches!(
            set_password(&conn, "sarita", Some("short")),
            Err(AccountError::ShortPassword)
        ));
        set_password(&conn, "sarita", Some("correct horse")).unwrap();
        verify_password("correct horse", &hash(&conn)).unwrap();
        assert!(
            crate::api_keys::authenticate_api_key(&conn, &key)
                .unwrap()
                .is_none()
        );

        let generated = set_password(&conn, "sarita", None).unwrap();
        assert_ne!(generated, made.password);
        verify_password(&generated, &hash(&conn)).unwrap();
    }
}
