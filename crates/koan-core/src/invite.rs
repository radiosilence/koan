//! Invite links: an account's server and username, and a token the app trades
//! for an API key of its own.
//!
//! The token is a JWT signed with the server's key, naming the account, a mark
//! of its password hash, and when it stops working. Nothing is stored when one
//! is made, so an admin can make another at any time, and one link signs in as
//! many devices as the account holder has until it expires, or until the
//! account's password changes, which changes the mark: a reset is how a link
//! sent to the wrong place is withdrawn. The server checks its own signature
//! when the app redeems it (`redeem`), and answers with a new API key: from
//! then on the app signs in the OpenSubsonic way, and every device it was
//! opened on has a key of its own to revoke.
//!
//! The server never keeps a password it can read back. A new account's
//! password is generated, shown once in the invite email for the web UI and
//! other Subsonic apps, and stored only as a hash.
//!
//! The link travels in the fragment of a koan.rocks address, which browsers
//! do not send, so the site serving the page never sees it. With the app
//! installed the address is a universal link and opens it directly; without,
//! the page offers the downloads and a `koan://join` button.
//!
//! A server address with the account in it (`https://user:password@host`),
//! which is what someone pasting into the server field may have, reads as an
//! invite too, and signs in with the password.
//!
//! The server never sends mail. It produces the email for the admin to send
//! from their own client.

use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use url::{Url, form_urlencoded};

use crate::auth::{self, Role};
use crate::db::queries::api_keys;
use crate::db::queries::auth as users;

pub const JOIN_PAGE: &str = "https://koan.rocks/join/";
pub const APP_STORE: &str = "https://apps.apple.com/app/id6817137172";
pub const MAC_DOWNLOAD: &str = "https://github.com/radiosilence/koan/releases/latest";

/// How long an invite link signs devices in for.
pub const TOKEN_TTL_SECS: u64 = 7 * 24 * 60 * 60;
const TOKEN_TYP: &str = "koan-invite";

const MAX_USERNAME: usize = 64;
const MIN_PASSWORD: usize = 8;
const MAX_DEVICE_NAME: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invite {
    pub server: String,
    pub username: String,
    /// What a koan server trades for an API key. Absent only from an address
    /// with the account in it.
    pub token: Option<String>,
    /// The account's password: generated with the account and put in the
    /// email once, or taken from an address. Never in a link.
    pub password: Option<String>,
}

impl Invite {
    /// An invite carrying `token`, and the account's password for the email
    /// when it was just made.
    pub fn with_token(server: &str, username: &str, token: &str, password: Option<&str>) -> Self {
        Self {
            server: server.trim().trim_end_matches('/').to_owned(),
            username: username.to_owned(),
            token: Some(token.to_owned()),
            password: password.map(str::to_owned),
        }
    }

    /// An account that signs in with its password: an address with the account
    /// in it.
    pub fn with_password(server: &str, username: &str, password: &str) -> Self {
        Self {
            server: server.trim().trim_end_matches('/').to_owned(),
            username: username.to_owned(),
            token: None,
            password: Some(password.to_owned()),
        }
    }

    fn params(&self) -> String {
        let mut p = form_urlencoded::Serializer::new(String::new());
        p.append_pair("server", &self.server)
            .append_pair("username", &self.username);
        if let Some(token) = &self.token {
            p.append_pair("invite", token);
        }
        p.finish()
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
    /// in it. Anything else, or a link missing a field, is `None`.
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
            return Some(Self::with_password(server.as_str(), &username, &password));
        }
        let params = match (url.scheme(), url.host_str()) {
            ("koan", Some("join")) => url.query().or(url.fragment()),
            ("https", Some("koan.rocks")) if url.path().trim_end_matches('/') == "/join" => {
                url.fragment()
            }
            _ => None,
        }?;
        let (mut server, mut username, mut token) = (None, None, None);
        for (k, v) in form_urlencoded::parse(params.as_bytes()) {
            let v = Some(v.into_owned()).filter(|v| !v.is_empty());
            match &*k {
                "server" => server = v,
                "username" => username = v,
                "invite" => token = v,
                _ => {}
            }
        }
        let (server, username) = (server?, username?);
        let scheme = Url::parse(&server).ok()?.scheme().to_owned();
        if !matches!(scheme.as_str(), "http" | "https") {
            return None;
        }
        Some(Self::with_token(&server, &username, &token?, None))
    }

    pub fn email_subject(&self) -> String {
        "Your koan account".to_owned()
    }

    /// How to sign in to anything that is not koan: the password when the
    /// email carries it, or where to make an API key when it does not.
    fn other_apps_text(&self) -> String {
        match &self.password {
            Some(password) => format!(
                "Using a different Subsonic app, or the web player at {server}? Sign in with:\n\
                 \n\
                 Server URL: {server}\n\
                 Username: {username}\n\
                 Password: {password}\n",
                server = self.server,
                username = self.username,
            ),
            None => format!(
                "Using a different Subsonic app? Sign in at {server} with your password and \
                 make an API key under API keys.\n",
                server = self.server,
            ),
        }
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
             koan signs in and loads the library by itself. The link works on each of \
             your devices for a week.\n\
             \n\
             {other}",
            link = self.link(),
            other = self.other_apps_text(),
        )
    }

    /// The same email with the link as a button, for pasting into a mail
    /// client as rich text.
    pub fn email_html(&self) -> String {
        let e = html_escape;
        let other = match &self.password {
            Some(password) => format!(
                "<p>Using a different Subsonic app, or the web player at {server}? Sign in \
                 with:</p><p>Server URL: {server}<br>Username: {username}<br>Password: \
                 {password}</p>",
                server = e(&self.server),
                username = e(&self.username),
                password = e(password),
            ),
            None => format!(
                "<p>Using a different Subsonic app? Sign in at {server} with your password \
                 and make an API key under API keys.</p>",
                server = e(&self.server),
            ),
        };
        format!(
            "<p>I've made you an account on my music server.</p>\
             <ol><li>Install koan: from the <a href=\"{APP_STORE}\">App Store</a> on an iPhone \
             or iPad, or <a href=\"{MAC_DOWNLOAD}\">for a Mac</a>.</li>\
             <li>On that device, open this link:</li></ol>\
             <p><a href=\"{link}\" style=\"display:inline-block;padding:10px 18px;\
             border-radius:8px;background:#111;color:#fff;text-decoration:none;\
             font-weight:600\">Open in koan</a></p>\
             <p>koan signs in and loads the library by itself. The link works on each of \
             your devices for a week.</p>{other}",
            link = e(&self.link()),
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

// ---------------------------------------------------------------------------
// Tokens
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct Claims {
    typ: String,
    /// The account's id. Ids are never reused, so a token outlives no
    /// account it was made for.
    sub: i64,
    username: String,
    /// `password_mark` of the account's password hash when the token was
    /// made.
    pwd: String,
    iat: u64,
    exp: u64,
}

/// Enough of a digest of the password hash to tell when it changed, and too
/// little to say anything about the password.
fn password_mark(password_hash: &str) -> String {
    auth::sha256_hex(password_hash)[..16].to_owned()
}

/// A token for the account `user_id`, signed with the server's private key.
pub fn mint_token(
    conn: &Connection,
    private_pem: &[u8],
    user_id: i64,
) -> Result<String, AccountError> {
    let user = users::get_user_by_id(conn, user_id)
        .map_err(other)?
        .ok_or_else(|| AccountError::NoSuchUser(user_id.to_string()))?;
    let now = auth::now_unix();
    let claims = Claims {
        typ: TOKEN_TYP.into(),
        sub: user.id,
        username: user.username,
        pwd: password_mark(&user.password_hash),
        iat: now,
        exp: now + TOKEN_TTL_SECS,
    };
    let key = EncodingKey::from_ed_pem(private_pem).map_err(other)?;
    jsonwebtoken::encode(&Header::new(Algorithm::EdDSA), &claims, &key).map_err(other)
}

/// What redeeming an invite gives the app: the account, and a key to sign in
/// with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redeemed {
    pub username: String,
    pub api_key: String,
}

/// Check `token` against the server's public key and make an API key for the
/// account it names, called `device`.
pub fn redeem(
    conn: &Connection,
    public_pem: &[u8],
    token: &str,
    device: &str,
) -> Result<Redeemed, AccountError> {
    let key = DecodingKey::from_ed_pem(public_pem).map_err(other)?;
    let mut validation = Validation::new(Algorithm::EdDSA);
    validation.set_required_spec_claims(&["exp"]);
    let claims = jsonwebtoken::decode::<Claims>(token, &key, &validation)
        .map_err(|_| AccountError::BadInvite)?
        .claims;
    if claims.typ != TOKEN_TYP {
        return Err(AccountError::BadInvite);
    }
    let user = users::get_user_by_id(conn, claims.sub)
        .map_err(other)?
        .filter(|u| u.username == claims.username)
        .filter(|u| password_mark(&u.password_hash) == claims.pwd)
        .ok_or(AccountError::BadInvite)?;
    let name: String = device
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_DEVICE_NAME)
        .collect();
    let name = if name.is_empty() { "koan" } else { &name };
    let (_, api_key) = api_keys::create_api_key(conn, user.id, name).map_err(other)?;
    Ok(Redeemed {
        username: user.username,
        api_key,
    })
}

// ---------------------------------------------------------------------------
// Accounts
// ---------------------------------------------------------------------------

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
    #[error(transparent)]
    Other(#[from] Box<dyn std::error::Error + Send + Sync>),
}

fn other(e: impl std::error::Error + Send + Sync + 'static) -> AccountError {
    AccountError::Other(Box::new(e))
}

/// An account just made: its id, for a token, and its generated password,
/// for the email. The password is not kept anywhere it can be read back.
#[derive(Debug)]
pub struct NewAccount {
    pub id: i64,
    pub password: String,
}

/// Create an account with a generated password.
pub fn create_account(
    conn: &Connection,
    username: &str,
    role: Role,
) -> Result<NewAccount, AccountError> {
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
    let id = users::create_user(conn, username, &password, role).map_err(other)?;
    Ok(NewAccount { id, password })
}

/// The account called `username`, or `NoSuchUser`.
pub fn account(conn: &Connection, username: &str) -> Result<users::UserRow, AccountError> {
    users::get_user_by_username(conn, username)
        .map_err(other)?
        .ok_or_else(|| AccountError::NoSuchUser(username.to_owned()))
}

/// Give an account `password`, or a generated one when `None`, which is
/// returned. Signs every device out: sessions end and API keys are revoked
/// (see `update_password`), invited devices included.
pub fn set_password(
    conn: &Connection,
    username: &str,
    password: Option<&str>,
) -> Result<String, AccountError> {
    account(conn, username)?;
    let password = match password {
        Some(p) if p.chars().count() < MIN_PASSWORD => return Err(AccountError::ShortPassword),
        Some(p) => p.to_owned(),
        None => generate_password().map_err(other)?,
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

    fn invite() -> Invite {
        Invite::with_token(
            "https://music.example.com/",
            "sarita",
            "a.b-c_d",
            Some("p&ss word=#?"),
        )
    }

    #[test]
    fn both_links_round_trip_without_the_password() {
        let i = invite();
        assert_eq!(i.server, "https://music.example.com");
        assert!(i.link().starts_with("https://koan.rocks/join/#server="));
        assert!(!i.link().contains("password"));
        let read = Invite {
            password: None,
            ..i.clone()
        };
        assert_eq!(Invite::parse(&i.link()), Some(read.clone()));
        assert_eq!(Invite::parse(&i.app_link()), Some(read));
    }

    #[test]
    fn links_carrying_a_password_are_not_invites() {
        let old =
            "https://koan.rocks/join/#server=https%3A%2F%2Fa.example&username=u&password=p%26q";
        assert_eq!(Invite::parse(old), None);
    }

    #[test]
    fn the_join_page_without_its_slash_still_parses() {
        let link = invite().link().replacen("/join/#", "/join#", 1);
        assert_eq!(
            Invite::parse(&link).unwrap().token.as_deref(),
            Some("a.b-c_d")
        );
    }

    #[test]
    fn an_address_with_the_account_in_it_is_split() {
        assert_eq!(
            Invite::parse("https://sarita:p%40ss+w@koan.example.com/"),
            Some(Invite::with_password(
                "https://koan.example.com",
                "sarita",
                "p@ss+w"
            ))
        );
        assert_eq!(Invite::parse("https://sarita@koan.example.com"), None);
    }

    #[test]
    fn other_links_and_missing_fields_are_refused() {
        assert_eq!(Invite::parse("https://example.com/join/#server=x"), None);
        assert_eq!(
            Invite::parse("https://koan.rocks/#server=https://a&username=u&invite=t"),
            None
        );
        assert_eq!(
            Invite::parse("koan://join?server=https://a&username=u"),
            None
        );
        assert_eq!(
            Invite::parse("koan://join?server=https://a&username=u&invite="),
            None
        );
        assert_eq!(
            Invite::parse("koan://join?server=ftp://a&username=u&invite=t"),
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
    fn the_email_carries_the_link_and_a_new_accounts_password() {
        let i = invite();
        let text = i.email_text();
        assert!(text.contains(&i.link()));
        assert!(text.contains("Server URL: https://music.example.com"));
        assert!(text.contains("Password: p&ss word=#?"));
        assert!(i.email_html().contains("p&amp;ss word=#?"));
        assert!(!i.mailto().contains(' '));

        let again = Invite {
            password: None,
            ..invite()
        };
        assert!(!again.email_text().contains("Password:"));
        assert!(again.email_text().contains("API key"));
    }

    #[test]
    fn generated_passwords_are_typeable() {
        let p = generate_password().unwrap();
        assert_eq!(p.len(), 20);
        assert!(!p.contains(['0', 'O', '1', 'l', 'I']));
        assert_ne!(p, generate_password().unwrap());
    }

    fn db() -> (tempfile::TempDir, crate::db::connection::Database) {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::connection::Database::open(&dir.path().join("t.db")).unwrap();
        (dir, db)
    }

    #[test]
    fn a_token_is_redeemed_for_a_key_per_device() {
        let (_dir, db) = db();
        let conn = &db.conn;
        let (private, public) = auth::generate_keypair_pem().unwrap();
        let made = create_account(conn, "sarita", Role::User).unwrap();
        let token = mint_token(conn, private.as_bytes(), made.id).unwrap();

        let phone = redeem(conn, public.as_bytes(), &token, "Sarita's iPhone").unwrap();
        let mac = redeem(conn, public.as_bytes(), &token, "").unwrap();
        assert_eq!(phone.username, "sarita");
        assert_ne!(phone.api_key, mac.api_key);
        let keys = api_keys::list_api_keys(conn, Some(made.id)).unwrap();
        let names: Vec<_> = keys.iter().map(|k| k.name.as_str()).collect();
        assert!(names.contains(&"Sarita's iPhone") && names.contains(&"koan"));
        assert!(
            api_keys::authenticate_api_key(conn, &phone.api_key)
                .unwrap()
                .is_some()
        );

        // Another server's key, a token for a deleted account, or something
        // that is not a token are all refused alike.
        let (_, elsewhere) = auth::generate_keypair_pem().unwrap();
        assert!(matches!(
            redeem(conn, elsewhere.as_bytes(), &token, "x"),
            Err(AccountError::BadInvite)
        ));
        assert!(matches!(
            redeem(conn, public.as_bytes(), "nonsense", "x"),
            Err(AccountError::BadInvite)
        ));
        let session =
            auth::mint_access_token(private.as_bytes(), made.id, "sarita", Role::User, 60).unwrap();
        assert!(matches!(
            redeem(conn, public.as_bytes(), &session, "x"),
            Err(AccountError::BadInvite)
        ));
        delete_account(conn, "sarita").unwrap();
        assert!(matches!(
            redeem(conn, public.as_bytes(), &token, "x"),
            Err(AccountError::BadInvite)
        ));
    }

    #[test]
    fn a_new_password_withdraws_links_already_sent() {
        let (_dir, db) = db();
        let conn = &db.conn;
        let (private, public) = auth::generate_keypair_pem().unwrap();
        let made = create_account(conn, "sarita", Role::User).unwrap();
        let sent = mint_token(conn, private.as_bytes(), made.id).unwrap();
        set_password(conn, "sarita", None).unwrap();
        assert!(matches!(
            redeem(conn, public.as_bytes(), &sent, "x"),
            Err(AccountError::BadInvite)
        ));
        let again = mint_token(conn, private.as_bytes(), made.id).unwrap();
        redeem(conn, public.as_bytes(), &again, "x").unwrap();
    }

    #[test]
    fn accounts_are_created_and_guarded() {
        let (_dir, db) = db();
        let conn = &db.conn;
        create_account(conn, "owner", Role::Admin).unwrap();
        assert!(matches!(
            create_account(conn, "owner", Role::User),
            Err(AccountError::Taken(_))
        ));
        assert!(matches!(
            create_account(conn, "two words", Role::User),
            Err(AccountError::BadUsername)
        ));
        assert!(matches!(
            create_account(conn, "anonymous", Role::User),
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
        assert!(matches!(
            set_password(conn, "nobody", None),
            Err(AccountError::NoSuchUser(_))
        ));
    }

    #[test]
    fn a_new_password_signs_every_device_out() {
        let (_dir, db) = db();
        let conn = &db.conn;
        let made = create_account(conn, "sarita", Role::Readonly).unwrap();
        let hash = |conn| account(conn, "sarita").unwrap().password_hash;
        auth::verify_password(&made.password, &hash(conn)).unwrap();
        let (_, key) = api_keys::create_api_key(conn, made.id, "phone").unwrap();

        assert!(matches!(
            set_password(conn, "sarita", Some("short")),
            Err(AccountError::ShortPassword)
        ));
        set_password(conn, "sarita", Some("correct horse")).unwrap();
        auth::verify_password("correct horse", &hash(conn)).unwrap();
        assert!(
            api_keys::authenticate_api_key(conn, &key)
                .unwrap()
                .is_none()
        );

        let generated = set_password(conn, "sarita", None).unwrap();
        assert_ne!(generated, made.password);
        auth::verify_password(&generated, &hash(conn)).unwrap();
    }
}
