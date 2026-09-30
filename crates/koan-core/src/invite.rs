//! Invite links: an account's server, username and password as one link.
//!
//! The link is the whole invitation. Nothing is redeemed on the server, so it
//! works against any Subsonic server and a mail scanner fetching it changes
//! nothing. The credentials travel in the fragment of a koan.rocks address,
//! which browsers do not send, so the site serving the page never sees them.
//! With the app installed the address is a universal link and opens it
//! directly; without, the page offers the downloads, a `koan://join` button
//! and the details in plain text for other clients.
//!
//! The server never sends mail. It produces the email for the admin to send
//! from their own client.

use rusqlite::Connection;
use url::{Url, form_urlencoded};

use crate::auth::{self, Role};
use crate::db::queries::auth as users;

pub const JOIN_PAGE: &str = "https://koan.rocks/join/";
pub const APP_STORE: &str = "https://apps.apple.com/app/id6817137172";
pub const MAC_DOWNLOAD: &str = "https://github.com/radiosilence/koan/releases/latest";

const MAX_USERNAME: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invite {
    pub server: String,
    pub username: String,
    pub password: String,
}

impl Invite {
    pub fn new(server: &str, username: &str, password: &str) -> Self {
        Self {
            server: server.trim().trim_end_matches('/').to_owned(),
            username: username.to_owned(),
            password: password.to_owned(),
        }
    }

    fn params(&self) -> String {
        form_urlencoded::Serializer::new(String::new())
            .append_pair("server", &self.server)
            .append_pair("username", &self.username)
            .append_pair("password", &self.password)
            .finish()
    }

    /// The link to send: a universal link into the app, or the join page.
    pub fn link(&self) -> String {
        format!("{JOIN_PAGE}#{}", self.params())
    }

    /// The app's own scheme, for where universal links do not reach.
    pub fn app_link(&self) -> String {
        format!("koan://join?{}", self.params())
    }

    /// Reads either form of the link, or a server address with the account
    /// in it (`https://user:password@host`), which is what someone pasting
    /// into the server field may have. Anything else, or a link missing a
    /// field, is `None`.
    pub fn parse(link: &str) -> Option<Self> {
        let url = Url::parse(link.trim()).ok()?;
        if matches!(url.scheme(), "http" | "https") && !url.username().is_empty() {
            let decode = |s: &str| percent_decode(s);
            let (username, password) = (decode(url.username()), decode(url.password()?));
            if password.is_empty() {
                return None;
            }
            let mut server = url;
            server.set_username("").ok()?;
            server.set_password(None).ok()?;
            return Some(Self::new(server.as_str(), &username, &password));
        }
        let params = match (url.scheme(), url.host_str()) {
            ("koan", Some("join")) => url.query().or(url.fragment()),
            ("https", Some("koan.rocks")) if url.path().trim_end_matches('/') == "/join" => {
                url.fragment()
            }
            _ => None,
        }?;
        let (mut server, mut username, mut password) = (None, None, None);
        for (k, v) in form_urlencoded::parse(params.as_bytes()) {
            match &*k {
                "server" => server = Some(v.into_owned()),
                "username" => username = Some(v.into_owned()),
                "password" => password = Some(v.into_owned()),
                _ => {}
            }
        }
        let (server, username, password) = (server?, username?, password?);
        let scheme = Url::parse(&server).ok()?.scheme().to_owned();
        if !matches!(scheme.as_str(), "http" | "https")
            || username.is_empty()
            || password.is_empty()
        {
            return None;
        }
        Some(Self::new(&server, &username, &password))
    }

    pub fn email_subject(&self) -> String {
        "Your koan account".to_owned()
    }

    pub fn email_text(&self) -> String {
        format!(
            "I've made you an account on my music server.\n\
             \n\
             1. Install koan: from the App Store on an iPhone or iPad ({APP_STORE}), \
             or for a Mac from {MAC_DOWNLOAD}\n\
             2. On that device, open this link:\n\
             \n\
             {link}\n\
             \n\
             koan signs in and loads the library by itself.\n\
             \n\
             Using a different Subsonic app? Sign in with:\n\
             \n\
             Server URL: {server}\n\
             Username: {username}\n\
             Password: {password}\n",
            link = self.link(),
            server = self.server,
            username = self.username,
            password = self.password,
        )
    }

    /// The same email with the link as a button, for pasting into a mail
    /// client as rich text.
    pub fn email_html(&self) -> String {
        let e = html_escape;
        format!(
            "<p>I've made you an account on my music server.</p>\
             <ol><li>Install koan: from the <a href=\"{APP_STORE}\">App Store</a> on an iPhone \
             or iPad, or <a href=\"{MAC_DOWNLOAD}\">for a Mac</a>.</li>\
             <li>On that device, open this link:</li></ol>\
             <p><a href=\"{link}\" style=\"display:inline-block;padding:10px 18px;\
             border-radius:8px;background:#111;color:#fff;text-decoration:none;\
             font-weight:600\">Open in koan</a></p>\
             <p>koan signs in and loads the library by itself.</p>\
             <p>Using a different Subsonic app? Sign in with:</p>\
             <p>Server URL: {server}<br>Username: {username}<br>Password: {password}</p>",
            link = e(&self.link()),
            server = e(&self.server),
            username = e(&self.username),
            password = e(&self.password),
        )
    }

    /// A `mailto:` with the subject and plain body filled in.
    pub fn mailto(&self) -> String {
        let enc = |s: &str| {
            form_urlencoded::byte_serialize(s.as_bytes())
                .collect::<String>()
                .replace('+', "%20")
        };
        format!(
            "mailto:?subject={}&body={}",
            enc(&self.email_subject()),
            enc(&self.email_text())
        )
    }
}

fn percent_decode(s: &str) -> String {
    form_urlencoded::parse(format!("x={}", s.replace('+', "%2B")).as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// A password someone can type from an email: 20 characters with no
/// lookalikes (0/O, 1/l/I), about 116 bits.
pub fn generate_password() -> Result<String, auth::AuthError> {
    use ring::rand::SecureRandom;

    const ALPHABET: &[u8] = b"abcdefghijkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let rng = ring::rand::SystemRandom::new();
    let mut out = String::with_capacity(20);
    let mut byte = [0u8; 1];
    while out.len() < 20 {
        rng.fill(&mut byte)
            .map_err(|_| auth::AuthError::Hash("rng failure".into()))?;
        // Reject the top of the range so every character is equally likely.
        let limit = 256 - 256 % ALPHABET.len();
        if (byte[0] as usize) < limit {
            out.push(ALPHABET[byte[0] as usize % ALPHABET.len()] as char);
        }
    }
    Ok(out)
}

#[derive(Debug, thiserror::Error)]
pub enum AccountError {
    #[error("usernames are 1 to {MAX_USERNAME} characters, without spaces")]
    BadUsername,
    #[error("there is already an account called {0}")]
    Taken(String),
    #[error("{0} is reserved")]
    Reserved(String),
    #[error("there is no account called {0}")]
    NoSuchUser(String),
    #[error("{0}'s password is not recoverable; invite with a new password instead")]
    NotRecoverable(String),
    #[error("the last admin cannot be removed or demoted")]
    LastAdmin,
    #[error(transparent)]
    Other(#[from] Box<dyn std::error::Error + Send + Sync>),
}

fn other(e: impl std::error::Error + Send + Sync + 'static) -> AccountError {
    AccountError::Other(Box::new(e))
}

fn seal(
    conn: &Connection,
    key: &[u8; 32],
    username: &str,
    password: &str,
) -> Result<(), AccountError> {
    let sealed = auth::seal_password(key, username, password).map_err(other)?;
    users::set_sealed_password(conn, username, &sealed).map_err(other)
}

/// Create an account with a generated password, sealed under `key` (the
/// server's `auth::subsonic_key`) so it can use token auth and be recovered
/// for a later invite. Returns the password.
pub fn create_account(
    conn: &Connection,
    key: &[u8; 32],
    username: &str,
    role: Role,
) -> Result<String, AccountError> {
    let username = username.trim();
    if username.is_empty()
        || username.chars().count() > MAX_USERNAME
        || username.chars().any(char::is_whitespace)
    {
        return Err(AccountError::BadUsername);
    }
    if username.eq_ignore_ascii_case(auth::ANONYMOUS) {
        return Err(AccountError::Reserved(username.to_owned()));
    }
    if users::get_user_by_username(conn, username)
        .map_err(other)?
        .is_some()
    {
        return Err(AccountError::Taken(username.to_owned()));
    }
    let password = generate_password().map_err(other)?;
    users::create_user(conn, username, &password, role).map_err(other)?;
    seal(conn, key, username, &password)?;
    Ok(password)
}

/// The password to put in an invite for an existing account.
///
/// Recovered from the sealed copy, so the account's other devices keep
/// working. With `reset`, a new password replaces it instead, which signs
/// every existing device out (see `update_password`); open links are the
/// server's to drop.
pub fn account_password(
    conn: &Connection,
    key: &[u8; 32],
    username: &str,
    reset: bool,
) -> Result<String, AccountError> {
    if users::get_user_by_username(conn, username)
        .map_err(other)?
        .is_none()
    {
        return Err(AccountError::NoSuchUser(username.to_owned()));
    }
    if !reset {
        return users::sealed_password(conn, username)
            .map_err(other)?
            .and_then(|sealed| auth::open_password(key, username, &sealed))
            .ok_or_else(|| AccountError::NotRecoverable(username.to_owned()));
    }
    let password = generate_password().map_err(other)?;
    users::update_password(conn, username, &password)
        .map_err(|e| AccountError::Other(e.to_string().into()))?;
    seal(conn, key, username, &password)?;
    Ok(password)
}

/// Change an account's role, refusing to demote the last admin.
pub fn set_role(conn: &Connection, username: &str, role: Role) -> Result<(), AccountError> {
    let user = users::get_user_by_username(conn, username)
        .map_err(other)?
        .ok_or_else(|| AccountError::NoSuchUser(username.to_owned()))?;
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
    let user = users::get_user_by_username(conn, username)
        .map_err(other)?
        .ok_or_else(|| AccountError::NoSuchUser(username.to_owned()))?;
    if user.role == Role::Admin && users::admin_count(conn).map_err(other)? <= 1 {
        return Err(AccountError::LastAdmin);
    }
    users::delete_user(conn, user.id).map_err(other)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invite() -> Invite {
        Invite::new("https://music.example.com/", "sarita", "p&ss word=#?")
    }

    #[test]
    fn both_links_round_trip() {
        let i = invite();
        assert_eq!(i.server, "https://music.example.com");
        assert!(i.link().starts_with("https://koan.rocks/join/#server="));
        assert_eq!(Invite::parse(&i.link()), Some(i.clone()));
        assert_eq!(Invite::parse(&i.app_link()), Some(i));
    }

    #[test]
    fn the_join_page_without_its_slash_still_parses() {
        let link = invite().link().replacen("/join/#", "/join#", 1);
        assert_eq!(Invite::parse(&link), Some(invite()));
    }

    #[test]
    fn an_address_with_the_account_in_it_is_split() {
        assert_eq!(
            Invite::parse("https://sarita:p%40ss+w@koan.example.com/"),
            Some(Invite::new("https://koan.example.com", "sarita", "p@ss+w"))
        );
        assert_eq!(Invite::parse("https://sarita@koan.example.com"), None);
    }

    #[test]
    fn other_links_and_missing_fields_are_refused() {
        assert_eq!(Invite::parse("https://example.com/join/#server=x"), None);
        assert_eq!(
            Invite::parse("https://koan.rocks/#server=https://a&username=u&password=p"),
            None
        );
        assert_eq!(
            Invite::parse("koan://join?server=https://a&username=u"),
            None
        );
        assert_eq!(
            Invite::parse("koan://join?server=ftp://a&username=u&password=p"),
            None
        );
        assert_eq!(Invite::parse("not a url"), None);
    }

    #[test]
    fn credentials_stay_in_the_fragment() {
        let url = Url::parse(&invite().link()).unwrap();
        assert_eq!(url.query(), None);
        assert!(!url.path().contains("sarita"));
    }

    #[test]
    fn the_email_carries_the_link_and_the_details() {
        let i = invite();
        let text = i.email_text();
        assert!(text.contains(&i.link()));
        assert!(text.contains("Server URL: https://music.example.com"));
        assert!(text.contains("Password: p&ss word=#?"));
        assert!(i.email_html().contains("p&amp;ss word=#?"));
        assert!(!i.mailto().contains(' '));
    }

    #[test]
    fn generated_passwords_are_typeable() {
        let p = generate_password().unwrap();
        assert_eq!(p.len(), 20);
        assert!(!p.contains(['0', 'O', '1', 'l', 'I']));
        assert_ne!(p, generate_password().unwrap());
    }

    #[test]
    fn accounts_are_created_recovered_and_guarded() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::connection::Database::open(&dir.path().join("t.db")).unwrap();
        let conn = &db.conn;
        let key = &[7; 32];
        let admin = create_account(conn, key, "owner", Role::Admin).unwrap();
        assert_eq!(account_password(conn, key, "owner", false).unwrap(), admin);
        assert!(matches!(
            create_account(conn, key, "owner", Role::User),
            Err(AccountError::Taken(_))
        ));
        assert!(matches!(
            create_account(conn, key, "two words", Role::User),
            Err(AccountError::BadUsername)
        ));
        assert!(matches!(
            create_account(conn, key, "anonymous", Role::User),
            Err(AccountError::Reserved(_))
        ));
        assert!(matches!(
            set_role(conn, "owner", Role::User),
            Err(AccountError::LastAdmin)
        ));
        assert!(matches!(
            delete_account(conn, "owner"),
            Err(AccountError::LastAdmin)
        ));

        let first = create_account(conn, key, "sarita", Role::Readonly).unwrap();
        let sarita = users::get_user_by_username(conn, "sarita")
            .unwrap()
            .unwrap()
            .id;
        let (_, api_key) =
            crate::db::queries::api_keys::create_api_key(conn, sarita, "phone").unwrap();
        // Recovering the password for an invite leaves the keys alone.
        account_password(conn, key, "sarita", false).unwrap();
        assert!(
            crate::db::queries::api_keys::authenticate_api_key(conn, &api_key)
                .unwrap()
                .is_some()
        );
        let reset = account_password(conn, key, "sarita", true).unwrap();
        assert_ne!(first, reset);
        assert_eq!(account_password(conn, key, "sarita", false).unwrap(), reset);
        // A reset takes the keys with the old password.
        assert!(
            crate::db::queries::api_keys::authenticate_api_key(conn, &api_key)
                .unwrap()
                .is_none()
        );
        delete_account(conn, "sarita").unwrap();
        assert!(matches!(
            account_password(conn, key, "sarita", false),
            Err(AccountError::NoSuchUser(_))
        ));
    }
}
