//! Subsonic errors: the codes clients act on, and how an error is answered.

use super::*;

#[derive(Debug, Clone, Copy)]
pub(super) enum SubsonicErrorCode {
    Generic = 0,
    MissingParameter = 10,
    WrongAuth = 40,
    TokenAuthUnsupported = 41,
    ConflictingAuth = 43,
    InvalidApiKey = 44,
    /// koan's own: the account has two-step sign-in, and its password was
    /// sent without a good second factor. `koanSignIn` takes one as `code`;
    /// nothing else does, so other requests sign in with a key instead.
    SecondFactor = 45,
    NotAuthorized = 50,
    NotFound = 70,
}

#[derive(Debug)]
pub(super) struct SubsonicError {
    pub(super) code: SubsonicErrorCode,
    pub(super) message: String,
    pub(super) help_url: Option<&'static str>,
}

impl SubsonicError {
    pub(super) fn new(code: SubsonicErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            help_url: None,
        }
    }

    pub(super) fn auth(code: SubsonicErrorCode, message: &str) -> Self {
        Self {
            help_url: Some(AUTH_HELP_URL),
            ..Self::new(code, message)
        }
    }

    pub(super) fn wrong_auth() -> Self {
        Self::new(SubsonicErrorCode::WrongAuth, "Wrong username or password")
    }

    /// Every password check slot was taken; see `auth::password`.
    pub(super) fn busy() -> Self {
        Self::new(
            SubsonicErrorCode::Generic,
            "Server busy checking passwords; try again shortly",
        )
    }

    /// The password was right, and the account wants its second factor too.
    /// `code` is what `koanSignIn` was given, if the request was one: empty
    /// when it was given none. `totp` is whether the account has an
    /// authenticator app; without one, only its keys or a recovery code will
    /// do, and an app can use no key.
    pub(super) fn second_factor(code: Option<&str>, totp: bool) -> Self {
        Self::auth(
            SubsonicErrorCode::SecondFactor,
            match code.map(str::trim) {
                Some("") if totp => {
                    "This account uses two-step sign-in: give a code from your authenticator app, or a recovery code"
                }
                Some("") => {
                    "This account's second factor is a security key, which apps cannot use: sign in with an invite or pairing from a signed-in device, or an API key. A recovery code works too, once each"
                }
                Some(_) => "Wrong code",
                None => {
                    "This account uses two-step sign-in, so its password alone does not sign in: sign in with an API key or an app password from the Account page of kōan's web UI"
                }
            },
        )
    }

    pub(super) fn token_auth_unsupported() -> Self {
        Self::auth(
            SubsonicErrorCode::TokenAuthUnsupported,
            "Token authentication needs an app password for this account: make one on the Account page of kōan's web UI, or sign in with your password or an API key",
        )
    }

    pub(super) fn conflicting_auth() -> Self {
        Self::auth(
            SubsonicErrorCode::ConflictingAuth,
            "Multiple conflicting authentication mechanisms provided",
        )
    }

    pub(super) fn invalid_api_key() -> Self {
        Self::auth(SubsonicErrorCode::InvalidApiKey, "Invalid API key")
    }

    pub(super) fn not_authorized() -> Self {
        Self::new(
            SubsonicErrorCode::NotAuthorized,
            "User is not authorized for the given operation",
        )
    }

    pub(super) fn missing_param(name: &str) -> Self {
        Self::new(
            SubsonicErrorCode::MissingParameter,
            format!("Required parameter '{}' is missing", name),
        )
    }

    pub(super) fn bad_param(name: &str) -> Self {
        Self::new(
            SubsonicErrorCode::MissingParameter,
            format!("Invalid value for parameter '{}'", name),
        )
    }

    pub(super) fn not_found(what: &str) -> Self {
        Self::new(SubsonicErrorCode::NotFound, format!("{} not found", what))
    }

    pub(super) fn unsupported(endpoint: &str) -> Self {
        Self::new(
            SubsonicErrorCode::NotFound,
            format!("Endpoint '{}' is not supported by this server", endpoint),
        )
    }

    /// A failure the client can do nothing about: the cause is logged and
    /// the client told only that it happened. SQLite's errors quote the
    /// failing statement and I/O errors quote absolute paths, a map of the
    /// host for whoever asked; GraphQL's `internal_error` keeps the same rule.
    pub(super) fn internal(cause: impl std::fmt::Display) -> Self {
        log::error!("subsonic: {cause}");
        Self::new(SubsonicErrorCode::Generic, "Internal error")
    }

    /// A generic failure whose message was written for the client.
    pub(super) fn generic(msg: impl Into<String>) -> Self {
        Self::new(SubsonicErrorCode::Generic, msg)
    }

    /// A playlist write's failure: a track or entry gone between being
    /// resolved and being written is the client's to hear, as GraphQL's
    /// `playlist_write_error` says it; anything else is internal.
    pub(super) fn playlist_write(e: koan_core::db::connection::DbError) -> Self {
        use koan_core::db::connection::DbError;
        match e {
            DbError::MissingTracks(_) | DbError::MissingEntry(_) => {
                Self::new(SubsonicErrorCode::NotFound, e.to_string())
            }
            e => Self::internal(e),
        }
    }
}

impl From<rusqlite::Error> for SubsonicError {
    fn from(e: rusqlite::Error) -> Self {
        Self::internal(e)
    }
}

impl IntoResponse for SubsonicError {
    fn into_response(self) -> Response {
        // Default to XML for error responses produced via `?` in handlers.
        SubsonicResponse::error(false, &self)
    }
}

#[cfg(test)]
mod tests {
    use crate::subsonic::testing::*;

    /// Every error line this test binary logs, so a test can show what went
    /// to the log rather than to the client.
    static LOGGED: parking_lot::Mutex<Vec<String>> = parking_lot::Mutex::new(Vec::new());

    struct CaptureLog;

    impl log::Log for CaptureLog {
        fn enabled(&self, _: &log::Metadata) -> bool {
            true
        }

        fn log(&self, record: &log::Record) {
            LOGGED.lock().push(record.args().to_string());
        }

        fn flush(&self) {}
    }

    /// Installs `CaptureLog` once for the binary; no other test sets a logger.
    fn capture_logs() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            log::set_logger(&CaptureLog).expect("no other test in this binary sets a logger");
            log::set_max_level(log::LevelFilter::Error);
        });
    }

    #[tokio::test]
    async fn a_database_error_reaches_the_client_as_a_generic_message_with_the_detail_only_in_the_log()
     {
        capture_logs();
        let (state, _dir) = test_state();
        // getGenres reads `tracks.genre`; without it SQLite's error quotes the
        // statement that failed.
        state
            .open_db()
            .unwrap()
            .conn
            .execute_batch("ALTER TABLE tracks RENAME COLUMN genre TO genre_gone")
            .unwrap();

        let app = build_test_router(state);
        let (_, body) = get_response(app, &format!("/rest/getGenres?{}", auth_query(""))).await;
        assert!(body.contains("status=\"failed\""), "{body}");
        assert!(body.contains("message=\"Internal error\""), "{body}");
        assert!(
            !body.contains("genre"),
            "SQLite's text reached the client: {body}"
        );
        assert!(
            LOGGED
                .lock()
                .iter()
                .any(|line| line.contains("no such column: genre")),
            "the cause was not logged"
        );
    }
}
