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
//! The token and the rules about accounts are `koan-auth`'s; the link, the
//! email and trading a token for a key are kōan's. A new account's password is
//! generated, shown once in the invite email for the web UI and other Subsonic
//! apps, and stored only as a hash.
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

use rusqlite::Connection;
use url::{Url, form_urlencoded};

use crate::auth::Role;
use crate::db::queries::api_keys;
use crate::db::queries::auth as users;

pub use koan_auth::accounts::{
    AccountError, NewAccount, account, delete_account, set_password, valid_username,
};
pub use koan_auth::api_keys::device_name;
pub use koan_auth::invite::{QR_TOKEN_TTL_SECS, TOKEN_TTL_SECS, mint_token};
pub use koan_auth::secret::generate_password;

pub const JOIN_PAGE: &str = "https://koan.rocks/join/";
pub const APP_STORE: &str = "https://apps.apple.com/app/id6817137172";
pub const MAC_DOWNLOAD: &str = "https://github.com/radiosilence/koan/releases/latest";

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
    /// A token good for `QR_TOKEN_TTL_SECS`, for the QR code: absent from a
    /// parsed link, and from servers older than it.
    pub qr_token: Option<String>,
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
            qr_token: None,
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
            qr_token: None,
        }
    }

    pub fn with_qr_token(self, qr_token: Option<&str>) -> Self {
        Self {
            qr_token: qr_token.map(str::to_owned),
            ..self
        }
    }

    fn params(&self, token: Option<&str>) -> String {
        let mut p = form_urlencoded::Serializer::new(String::new());
        p.append_pair("server", &self.server)
            .append_pair("username", &self.username);
        if let Some(token) = token {
            p.append_pair("invite", token);
        }
        p.finish()
    }

    /// The link to send: a universal link into the app, or the join page.
    pub fn link(&self) -> String {
        format!("{JOIN_PAGE}#{}", self.params(self.token.as_deref()))
    }

    /// The link to show as a QR code: anyone who sees the screen, or a
    /// recording of it, can scan one, so it carries the short-lived token
    /// where there is one.
    pub fn qr_link(&self) -> String {
        let token = self.qr_token.as_deref().or(self.token.as_deref());
        format!("{JOIN_PAGE}#{}", self.params(token))
    }

    /// The app's own scheme, for where universal links do not reach.
    pub fn app_link(&self) -> String {
        format!("koan://join?{}", self.params(self.token.as_deref()))
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
// Redeeming, and accounts with kōan's implicit user
// ---------------------------------------------------------------------------

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
    let user = koan_auth::invite::invited(conn, public_pem, token)?;
    let (_, api_key) = api_keys::create_api_key(conn, user.id, &device_name(device))?;
    Ok(Redeemed {
        username: user.username,
        api_key,
    })
}

/// Create an account with a generated password. The first admin takes over
/// what the implicit user had (`users::adopt_local_rows`).
pub fn create_account(
    conn: &Connection,
    username: &str,
    role: Role,
) -> Result<NewAccount, AccountError> {
    let made = koan_auth::accounts::create_account(conn, username, role)?;
    users::adopt_local_rows(conn)?;
    Ok(made)
}

/// Make the first admin of a server that has no accounts, with the password
/// they chose: the web UI's setup page.
pub fn create_first_admin(
    conn: &Connection,
    username: &str,
    password: &str,
) -> Result<i64, AccountError> {
    let id = koan_auth::accounts::create_first_admin(conn, username, password)?;
    users::adopt_local_rows(conn)?;
    Ok(id)
}

/// Change an account's role, refusing to demote the last admin.
pub fn set_role(conn: &Connection, username: &str, role: Role) -> Result<(), AccountError> {
    koan_auth::accounts::set_role(conn, username, role)?;
    users::adopt_local_rows(conn)?;
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
    fn the_qr_link_carries_the_short_token_where_there_is_one() {
        let i = invite();
        assert_eq!(i.qr_link(), i.link());
        let shown = i.clone().with_qr_token(Some("short"));
        assert_eq!(shown.link(), i.link());
        assert_eq!(
            Invite::parse(&shown.qr_link()).and_then(|r| r.token),
            Some("short".into())
        );
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
        let (private, public) = crate::auth::generate_keypair_pem().unwrap();
        let made = create_account(conn, "sarita", Role::User).unwrap();
        let token = mint_token(conn, private.as_bytes(), made.id, TOKEN_TTL_SECS).unwrap();

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
        let (_, elsewhere) = crate::auth::generate_keypair_pem().unwrap();
        assert!(matches!(
            redeem(conn, elsewhere.as_bytes(), &token, "x"),
            Err(AccountError::BadInvite)
        ));
        assert!(matches!(
            redeem(conn, public.as_bytes(), "nonsense", "x"),
            Err(AccountError::BadInvite)
        ));
        let session =
            crate::auth::mint_access_token(private.as_bytes(), made.id, "sarita", Role::User, 60)
                .unwrap();
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
}
