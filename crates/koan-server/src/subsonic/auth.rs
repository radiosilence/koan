//! Who a request signs in as: the credential checks, two-step sign-in, app
//! passwords, and the throttle on failed sign-ins.

use super::*;

/// Marks a response that refused a credential, for [`throttle_auth`] to count.
#[derive(Clone, Copy)]
pub(super) struct AuthFailed;

/// Failed sign-ins allowed per client and username in a minute.
pub(super) const AUTH_FAILURES_PER_MINUTE: u32 = 10;
/// Failed sign-ins allowed per client, whatever the username, in a minute.
pub(super) const AUTH_FAILURES_PER_ADDRESS_PER_MINUTE: u32 = 30;

/// Failed sign-ins counted three ways. An address is an IPv6 client's /64
/// (see `routes::network`).
///
/// By client address and username: behind a relay that does not pass
/// addresses on, every outside client arrives from the relay's, and a tight
/// limit on that alone would let anyone lock everyone out. With the username
/// in the key, guessing one account's password throttles that account's
/// password sign-ins and nothing else.
///
/// By address alone, with a looser limit: otherwise a new username per request
/// gets a fresh allowance every time.
///
/// By username alone, from every address and every door that takes a
/// password, with the loosest: the verifier's `failures`.
pub(super) struct AuthThrottle {
    pub(super) per_account: FailureLimiter<(std::net::IpAddr, String)>,
    pub(super) per_address: FailureLimiter<std::net::IpAddr>,
    pub(super) users: Arc<PasswordVerifier>,
    /// The `[subsonic]` user, whose token is checked against a random 256-bit
    /// secret rather than a password. `None` without a secret: the name is then
    /// an ordinary account's, and its tokens are checked against its password.
    pub(super) shared_username: Option<String>,
}

impl AuthThrottle {
    pub(super) fn new(shared_username: Option<String>, users: Arc<PasswordVerifier>) -> Self {
        Self {
            per_account: FailureLimiter::new(AUTH_FAILURES_PER_MINUTE),
            per_address: FailureLimiter::new(AUTH_FAILURES_PER_ADDRESS_PER_MINUTE),
            users,
            shared_username,
        }
    }
}

/// Refuse password sign-ins from a client that has failed too often, and
/// count failures.
///
/// Every request carries its credential, so each is a sign-in: a password
/// through argon2, or a token against an account's sealed password, which is
/// only an MD5 and cheap to try. Only refusals count, so a client syncing a
/// library is never slowed. API keys and the shared secret's token are random
/// and not worth guessing, so they are never throttled: a flood of wrong
/// passwords for an account cannot lock out the apps signed in with either.
///
/// The exemption follows `validate_auth` exactly. A request with an API key
/// has no other credential checked; one for the shared username with both `t`
/// and `s` is checked against the secret alone. Anything else — `t` without
/// `s` falls through to `p` — is a password sign-in and counted.
pub(super) async fn throttle_auth(
    State(throttle): State<Arc<AuthThrottle>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let params = RawParams::parse(request_params(request.uri(), request.extensions()).as_deref());
    let username = params.get("u").unwrap_or_default().to_owned();
    let unguessable = params.get("apiKey").is_some()
        || (params.get("t").is_some()
            && params.get("s").is_some()
            && throttle.shared_username.as_deref() == Some(username.as_str()));
    if unguessable {
        return next.run(request).await;
    }

    let from = crate::auth::routes::client_ip(&request);
    let ip = crate::auth::routes::network(from);
    let key = (ip, username);
    let refusal = if throttle.per_account.exhausted(&key) {
        Some("Too many failed sign-ins for this account from this address; try again in a minute")
    } else if throttle.per_address.exhausted(&ip) {
        Some("Too many failed sign-ins from this address; try again in a minute")
    } else if throttle.users.spent(&key.1, from) {
        Some("Too many failed sign-ins for this account; try again in a minute")
    } else {
        None
    };
    if let Some(message) = refusal {
        let json = params.get("f") == Some("json");
        return SubsonicResponse::error(
            json,
            &SubsonicError::new(SubsonicErrorCode::Generic, message),
        );
    }
    let response = next.run(request).await;
    if response.extensions().get::<AuthFailed>().is_some() {
        throttle.users.failed(&key.1);
        throttle.per_account.record(key);
        throttle.per_address.record(ip);
    } else if throttle.users.took_pass(&credential_digest(&params.auth())) {
        throttle.users.signed_in(&key.1, from);
    }
    response
}

/// The credential a request signs in with besides an API key, as
/// `validate_auth` and `throttle_auth` both name it.
pub(super) fn credential_digest(auth: &SubsonicParams) -> [u8; 32] {
    use sha2::Digest as _;
    let mut digest = sha2::Sha256::new();
    for part in [&auth.u, &auth.p, &auth.t, &auth.s] {
        match part {
            Some(v) => digest.update([&[1u8][..], v.as_bytes(), &[0]].concat()),
            None => digest.update([0u8]),
        }
    }
    digest.finalize().into()
}

/// Who a request acts as.
pub(super) struct Caller {
    pub(super) username: String,
    pub(super) role: Role,
    /// Whose favourites, playlists and history the request reads and writes:
    /// the account's, or the local user's for the shared secret.
    pub(super) user_id: i64,
    /// What signed the request.
    pub(super) via: Via,
}

/// How a request authenticated, for the endpoints that care.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Via {
    ApiKey,
    /// The account's own password, as `p=`.
    Password,
    AppPassword,
    /// The `[subsonic]` shared secret, which is no account's.
    SharedSecret,
}

impl Caller {
    /// Refuse a request about the account's own credentials, its password and
    /// API keys, unless it is signed with one of them: an API key or the
    /// account's password. An app password is a credential handed to one
    /// client, which must not mint or revoke others; the shared secret is no
    /// account's.
    pub(super) fn may_manage_credentials(&self) -> Result<(), SubsonicError> {
        match self.via {
            Via::ApiKey | Via::Password => Ok(()),
            Via::AppPassword | Via::SharedSecret => Err(SubsonicError::new(
                SubsonicErrorCode::NotAuthorized,
                "sign in with the account's password or an API key to manage its credentials",
            )),
        }
    }

    /// Whose shares the caller may list and change: their own, or everyone's
    /// for an admin.
    pub(super) fn share_owner(&self) -> Option<i64> {
        (self.role != Role::Admin).then_some(self.user_id)
    }
}

/// Authenticate a request.
///
/// Three ways in:
/// - `apiKey=`, a key made for a koan account (`koan auth api-key create`, or
///   the web UI's keys page). It names its user, so `u` alongside it is a
///   conflict (43), as is any other credential.
/// - `p=` (plain or `enc:` hex) with a koan account's password, checked
///   against its argon2 hash. The protocol sends it with every request, so it
///   is only as private as the transport. The web login form sends it too.
/// - `t=md5(secret + s)` with the `[subsonic]` shared secret, for clients that
///   only speak token auth. The secret acts as `User`, and is also accepted as
///   `p=`.
/// - `t=md5(app password + s)`, or `p=` with an app password: generated per
///   client for an account, kept sealed (`queries::app_passwords`). Token auth
///   cannot work against the account's own password, which is only a hash, so
///   an account without app passwords is refused with 41, the code that tells
///   a client to fall back to a password or a key.
///
/// A password or app password that signs in is noted for `throttle_auth`,
/// which marks the client's network as the account's.
pub(super) fn validate_auth(
    params: &SubsonicParams,
    state: &AppState,
) -> Result<Caller, SubsonicError> {
    let caller = check_credential(params, state)?;
    if params.api_key.is_none() && caller.user_id != queries::LOCAL_USER {
        state.users.passed(credential_digest(params));
    }
    Ok(caller)
}

pub(super) fn check_credential(
    params: &SubsonicParams,
    state: &AppState,
) -> Result<Caller, SubsonicError> {
    use crate::auth::password::Refused;
    use subtle::ConstantTimeEq;

    if let Some(key) = params.api_key.as_deref() {
        if params.u.is_some() || params.p.is_some() || params.t.is_some() || params.s.is_some() {
            return Err(SubsonicError::conflicting_auth());
        }
        let db = state.open_db()?;
        let host = state
            .host_key
            .as_deref()
            .is_some_and(|host| bool::from(key.as_bytes().ct_eq(host.as_bytes())));
        let user = if host {
            queries::auth::first_admin(&db.conn)
                .and_then(|id| match id {
                    Some(id) => queries::auth::get_user_by_id(&db.conn, id),
                    None => Ok(None),
                })
                .map_err(|e| SubsonicError::internal(e.to_string()))?
        } else {
            queries::api_keys::authenticate_api_key(&db.conn, key)
                .map_err(|e| SubsonicError::internal(e.to_string()))?
        }
        .ok_or_else(SubsonicError::invalid_api_key)?;
        return Ok(Caller {
            username: user.username,
            role: user.role,
            user_id: user.id,
            via: Via::ApiKey,
        });
    }

    let username = params
        .u
        .as_deref()
        .ok_or_else(|| SubsonicError::missing_param("u"))?;
    let caller = |(user_id, role), via| {
        Ok(Caller {
            username: username.to_owned(),
            role,
            user_id,
            via,
        })
    };
    let shared = |given: &str| {
        let user_ok = username.as_bytes().ct_eq(state.username.as_bytes());
        state
            .password
            .as_deref()
            .is_some_and(|p| bool::from(user_ok & given.as_bytes().ct_eq(p.as_bytes())))
    };

    if let (Some(token), Some(salt)) = (params.t.as_deref(), params.s.as_deref()) {
        if let Some(secret) = state
            .password
            .as_deref()
            .filter(|_| username == state.username)
        {
            let expected = format!("{:x}", md5::compute(format!("{secret}{salt}")));
            return if bool::from(token.as_bytes().ct_eq(expected.as_bytes())) {
                caller((queries::LOCAL_USER, Role::User), Via::SharedSecret)
            } else {
                Err(SubsonicError::wrong_auth())
            };
        }
        // An account: its own password is only a hash, so a token is checked
        // against its app passwords, and without any, 41 tells the client to
        // send the password or a key instead.
        return match app_password(state, username, |password| {
            let expected = format!("{:x}", md5::compute(format!("{password}{salt}")));
            bool::from(token.as_bytes().ct_eq(expected.as_bytes()))
        })? {
            AppPasswordAuth::Matched(user) => Ok(Caller {
                username: user.username,
                role: user.role,
                user_id: user.id,
                via: Via::AppPassword,
            }),
            AppPasswordAuth::Wrong => Err(SubsonicError::wrong_auth()),
            AppPasswordAuth::NoneMade => Err(SubsonicError::token_auth_unsupported()),
        };
    }

    let Some(p) = params.p.as_deref() else {
        return Err(SubsonicError::missing_param("t and s"));
    };
    let password = match p.strip_prefix("enc:") {
        Some(hex) => decode_hex(hex).ok_or_else(SubsonicError::wrong_auth)?,
        None => p.to_string(),
    };
    if shared(&password) {
        return caller((queries::LOCAL_USER, Role::User), Via::SharedSecret);
    }
    // An app password costs a decryption to check, the account's own an
    // argon2 hash, so the cheaper goes first.
    let given = password.as_bytes();
    if let AppPasswordAuth::Matched(user) =
        app_password(state, username, |p| bool::from(p.as_bytes().ct_eq(given)))?
    {
        return Ok(Caller {
            username: user.username,
            role: user.role,
            user_id: user.id,
            via: Via::AppPassword,
        });
    }
    let account = match state.users.verify(username, &password) {
        Ok(account) => account,
        Err(Refused::Busy) => return Err(SubsonicError::busy()),
        Err(Refused::Wrong) => return Err(SubsonicError::wrong_auth()),
    };
    second_factor(state, &account, params.code.as_deref())?;
    caller((account.id, account.role), Via::Password)
}

/// Refuse a right password for an account with two-step sign-in unless
/// `code` is its second factor. Only `koanSignIn` passes one; a wrong code
/// counts against the account's budget for codes, and is not a wrong
/// password, which the sign-in throttle would count again.
pub(super) fn second_factor(
    state: &AppState,
    account: &queries::auth::UserRow,
    code: Option<&str>,
) -> Result<(), SubsonicError> {
    let db = state.open_db()?;
    let factors = koan_auth::second_factor::factors(&db.conn, account.id)
        .map_err(|e| SubsonicError::internal(e.to_string()))?;
    if !factors.enabled() {
        return Ok(());
    }
    let refused = || SubsonicError::second_factor(code, factors.totp);
    let Some(given) = code.filter(|c| !c.trim().is_empty()) else {
        return Err(refused());
    };
    if state.users.codes_spent(&account.username) {
        return Err(SubsonicError::generic(
            "Too many wrong codes for this account; try again in a minute",
        ));
    }
    let key = state.totp_key.as_ref().ok_or_else(refused)?;
    use koan_auth::second_factor::Checked;
    match koan_auth::second_factor::check_code(&db.conn, key, account.id, given)
        .map_err(|e| SubsonicError::internal(e.to_string()))?
    {
        Checked::Right => Ok(()),
        Checked::Wrong => {
            state.users.code_failed(&account.username);
            Err(refused())
        }
        Checked::Locked => Err(SubsonicError::generic(
            "Too many wrong codes in a row: sign in to the web UI with a passkey, or ask an admin to turn two-step sign-in off",
        )),
    }
}

/// `username`'s app passwords, checked with `matches`.
pub(super) fn app_password(
    state: &AppState,
    username: &str,
    matches: impl Fn(&str) -> bool,
) -> Result<AppPasswordAuth, SubsonicError> {
    let Some(key) = state.app_key.as_ref() else {
        return Ok(AppPasswordAuth::NoneMade);
    };
    let db = state.open_db()?;
    queries::app_passwords::authenticate_app_password(&db.conn, key, username, matches)
        .map_err(|e| SubsonicError::internal(e.to_string()))
}

pub(super) fn decode_hex(hex: &str) -> Option<String> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    let bytes = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect::<Option<Vec<u8>>>()?;
    String::from_utf8(bytes).ok()
}

/// Refuse a readonly account the endpoints that change anything.
pub(super) fn require_write(role: Role) -> Result<(), SubsonicError> {
    if role.has_permission(Role::User) {
        Ok(())
    } else {
        Err(SubsonicError::not_authorized())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subsonic::testing::*;
    use axum::http::Request;

    #[tokio::test]
    async fn test_failed_password_sign_ins_are_throttled_per_account() {
        let (state, _dir) = test_state();
        let app = subsonic_app(state);
        let wrong = "/rest/ping?u=testuser&p=wrong&v=1.16.1&c=test";

        // Successes never count.
        for _ in 0..20 {
            let (_, body) =
                get_response(app.clone(), &format!("/rest/ping?{}", auth_query(""))).await;
            assert!(body.contains("status=\"ok\""), "{}", body);
        }
        for _ in 0..10 {
            let (_, body) = get_response(app.clone(), wrong).await;
            assert!(body.contains("code=\"40\""), "{}", body);
        }
        // Spent: password sign-ins for this account are refused, even correct
        // ones, for the rest of the minute.
        let (_, body) = get_response(
            app.clone(),
            "/rest/ping?u=testuser&p=testpass&v=1.16.1&c=test",
        )
        .await;
        assert!(body.contains("Too many failed sign-ins"), "{}", body);
        // The shared secret's token is not a password and is not throttled,
        // so the apps signed in with it keep working.
        let (_, body) = get_response(app.clone(), &format!("/rest/ping?{}", auth_query(""))).await;
        assert!(body.contains("status=\"ok\""), "{}", body);
        // Nor is another account.
        let (_, body) = get_response(app, "/rest/ping?u=someone&p=wrong&v=1.16.1&c=test").await;
        assert!(body.contains("code=\"40\""), "{}", body);
    }

    /// A route that refuses every credential, behind the throttle.
    fn refusing_app(shared_username: Option<&str>) -> axum::Router {
        async fn refuse() -> Response {
            SubsonicResponse::error(false, &SubsonicError::wrong_auth())
        }
        axum::Router::new().route("/rest/ping", get(refuse)).layer(
            axum::middleware::from_fn_with_state(
                Arc::new(AuthThrottle::new(
                    shared_username.map(str::to_owned),
                    Arc::new(PasswordVerifier::new(Arc::new(Pool::new(
                        "/nonexistent/koan.db".into(),
                    )))),
                )),
                throttle_auth,
            ),
        )
    }

    #[tokio::test]
    async fn failures_are_also_counted_per_address_whatever_the_username() {
        let app = refusing_app(None);
        for n in 0..AUTH_FAILURES_PER_ADDRESS_PER_MINUTE {
            let (_, body) = get_response(app.clone(), &format!("/rest/ping?u=user{n}&p=x")).await;
            assert!(body.contains("code=\"40\""), "{}", body);
        }
        let (_, body) = get_response(app, "/rest/ping?u=fresh&p=x").await;
        assert!(
            body.contains("Too many failed sign-ins from this address"),
            "{}",
            body
        );
    }

    #[tokio::test]
    async fn a_token_without_a_salt_is_a_password_sign_in() {
        // `validate_auth` checks `p` when `s` is missing, so the shared
        // username's exemption must not cover it.
        let guess = "/rest/ping?u=koan&t=0123&p=guess";
        let app = refusing_app(Some("koan"));
        for _ in 0..AUTH_FAILURES_PER_MINUTE {
            let (_, body) = get_response(app.clone(), guess).await;
            assert!(body.contains("code=\"40\""), "{}", body);
        }
        let (_, body) = get_response(app, guess).await;
        assert!(body.contains("Too many failed sign-ins"), "{}", body);
    }

    #[tokio::test]
    async fn failures_are_also_counted_per_username_from_every_address() {
        let app = refusing_app(None);
        let from = |n: u32, query: &str| {
            let mut request = Request::builder()
                .uri(format!("/rest/ping?{query}"))
                .body(Body::empty())
                .unwrap();
            request.extensions_mut().insert(axum::extract::ConnectInfo(
                std::net::SocketAddr::from(([198, 51, (n / 256) as u8, (n % 256) as u8], 1234)),
            ));
            request
        };
        let body = |response: Response| async {
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            String::from_utf8_lossy(&bytes).into_owned()
        };
        // One guess from each address stays under every per-address limit.
        for n in 0..crate::auth::password::FAILURES_PER_USERNAME_PER_MINUTE {
            let response = app.clone().oneshot(from(n, "u=mate&p=x")).await.unwrap();
            assert!(body(response).await.contains("code=\"40\""));
        }
        let response = app.clone().oneshot(from(999, "u=mate&p=x")).await.unwrap();
        assert!(
            body(response)
                .await
                .contains("Too many failed sign-ins for this account;")
        );
        // Another account, and an API key, are untouched.
        let response = app.clone().oneshot(from(999, "u=other&p=x")).await.unwrap();
        assert!(body(response).await.contains("code=\"40\""));
        let response = app.oneshot(from(999, "apiKey=k")).await.unwrap();
        assert!(body(response).await.contains("code=\"40\""));
    }

    #[tokio::test]
    async fn a_spent_username_budget_spares_the_accounts_own_network() {
        let (state, _dir) = test_state();
        let users = state.users.clone();
        let app = subsonic_app(state);
        let from = |ip: [u8; 4]| {
            let mut request = Request::builder()
                .uri("/rest/ping?u=mate&p=hunter22&v=1.16.1&c=test")
                .body(Body::empty())
                .unwrap();
            request.extensions_mut().insert(axum::extract::ConnectInfo(
                std::net::SocketAddr::from((ip, 1234)),
            ));
            request
        };
        let body = |response: Response| async {
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            String::from_utf8_lossy(&bytes).into_owned()
        };
        let home = [198, 51, 100, 4];
        let r = body(app.clone().oneshot(from(home)).await.unwrap()).await;
        assert!(r.contains("status=\"ok\""), "{r}");
        for _ in 0..crate::auth::password::FAILURES_PER_USERNAME_PER_MINUTE {
            users.failed("mate");
        }
        let r = body(app.clone().oneshot(from([203, 0, 113, 9])).await.unwrap()).await;
        assert!(
            r.contains("Too many failed sign-ins for this account;"),
            "{r}"
        );
        let r = body(app.oneshot(from(home)).await.unwrap()).await;
        assert!(r.contains("status=\"ok\""), "{r}");
    }

    #[tokio::test]
    async fn an_ipv6_client_is_throttled_by_its_64() {
        let app = refusing_app(None);
        for n in 0..AUTH_FAILURES_PER_ADDRESS_PER_MINUTE {
            let mut request = Request::builder()
                .uri(format!("/rest/ping?u=user{n}&p=x"))
                .body(Body::empty())
                .unwrap();
            let ip: std::net::Ipv6Addr = format!("2001:db8:1:2::{n:x}").parse().unwrap();
            request.extensions_mut().insert(axum::extract::ConnectInfo(
                std::net::SocketAddr::from((ip, 1234)),
            ));
            app.clone().oneshot(request).await.unwrap();
        }
        let mut request = Request::builder()
            .uri("/rest/ping?u=fresh&p=x")
            .body(Body::empty())
            .unwrap();
        let ip: std::net::Ipv6Addr = "2001:db8:1:2:abcd::1".parse().unwrap();
        request
            .extensions_mut()
            .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                ip, 1234,
            ))));
        let response = app.oneshot(request).await.unwrap();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("from this address"));
    }

    #[tokio::test]
    async fn the_shared_username_is_exempt_only_with_a_secret() {
        let guess = "/rest/ping?u=koan&t=0123&s=salt";
        let app = refusing_app(None);
        for _ in 0..AUTH_FAILURES_PER_MINUTE {
            get_response(app.clone(), guess).await;
        }
        let (_, body) = get_response(app, guess).await;
        assert!(body.contains("Too many failed sign-ins"), "{}", body);

        let app = refusing_app(Some("koan"));
        for _ in 0..AUTH_FAILURES_PER_ADDRESS_PER_MINUTE + 1 {
            let (_, body) = get_response(app.clone(), guess).await;
            assert!(body.contains("code=\"40\""), "{}", body);
        }
    }

    #[tokio::test]
    async fn test_ping_wrong_password() {
        let (state, _dir) = test_state();
        let app = build_test_router(state);
        let (status, body) = get_response(
            app,
            "/rest/ping?u=testuser&t=wrongtoken&s=abc&v=1.16.1&c=test",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("status=\"failed\""));
        assert!(body.contains("code=\"40\""));
    }

    /// Token auth for any username but the shared secret's is 41, which tells
    /// a client to fall back to a password or an API key.
    #[tokio::test]
    async fn test_token_auth_for_other_users_is_41() {
        let (state, _dir) = test_state();
        let app = build_test_router(state);
        let salt = "abc123";
        let token = format!("{:x}", md5::compute(format!("testpass{}", salt)));
        let (_, body) = get_response(
            app,
            &format!(
                "/rest/ping?u=wronguser&t={}&s={}&v=1.16.1&c=test",
                token, salt
            ),
        )
        .await;
        assert!(body.contains("status=\"failed\""));
        assert!(body.contains("code=\"41\""), "{body}");
        assert!(
            body.contains(&format!("helpUrl=\"{AUTH_HELP_URL}\"")),
            "{body}"
        );
    }

    #[tokio::test]
    async fn an_app_password_signs_an_account_in_by_token_or_as_a_password() {
        let (state, _dir) = test_state();
        let ping = |q: String| {
            let app = build_test_router(state.clone());
            async move {
                get_response(app, &format!("/rest/ping?{q}&v=1.16.1&c=test"))
                    .await
                    .1
            }
        };
        let token =
            |secret: &str, salt: &str| format!("{:x}", md5::compute(format!("{secret}{salt}")));

        // No app password yet: 41, which tells the client to fall back.
        let body = ping(format!("u=mate&t={}&s=abc", token("hunter22", "abc"))).await;
        assert!(body.contains("code=\"41\""), "{body}");

        let password = {
            let db = state.open_db().unwrap();
            let mate = queries::auth::get_user_by_username(&db.conn, "mate")
                .unwrap()
                .unwrap();
            queries::app_passwords::create_app_password(
                &db.conn,
                state.app_key.as_ref().unwrap(),
                mate.id,
                "arpeggi",
            )
            .unwrap()
            .1
        };
        let body = ping(format!("u=mate&t={}&s=abc", token(&password, "abc"))).await;
        assert!(body.contains("status=\"ok\""), "{body}");
        let body = ping(format!("u=mate&p={password}")).await;
        assert!(body.contains("status=\"ok\""), "{body}");
        // The account's own password still does not work as a token, and a
        // wrong token is a wrong credential now that app passwords exist.
        let body = ping(format!("u=mate&t={}&s=abc", token("hunter22", "abc"))).await;
        assert!(body.contains("code=\"40\""), "{body}");
    }

    #[tokio::test]
    async fn test_account_password_auth() {
        let (state, _dir) = test_state();
        // "hunter22" as enc: hex.
        for q in ["p=hunter22", "p=enc:68756e7465723232"] {
            let (_, body) = get_response(
                build_test_router(state.clone()),
                &format!("/rest/ping?u=mate&{q}&v=1.16.1&c=test"),
            )
            .await;
            assert!(body.contains("status=\"ok\""), "{q}: {body}");
        }
        let (_, body) = get_response(
            build_test_router(state),
            "/rest/ping?u=mate&p=wrong&v=1.16.1&c=test",
        )
        .await;
        assert!(body.contains("code=\"40\""));
    }

    #[tokio::test]
    async fn test_shared_secret_as_password() {
        let (state, _dir) = test_state();
        let (_, body) = get_response(
            build_test_router(state),
            "/rest/ping?u=testuser&p=enc:7465737470617373&v=1.16.1&c=test",
        )
        .await;
        assert!(body.contains("status=\"ok\""));
    }

    #[tokio::test]
    async fn test_readonly_account_cannot_write() {
        let (state, _dir) = test_state();
        let (_, body) = get_response(
            build_test_router(state.clone()),
            "/rest/star?id=mf-1&u=mate&p=hunter22&v=1.16.1&c=test",
        )
        .await;
        assert!(body.contains("code=\"50\""), "{body}");
        // Controlling a device is a write, as GraphQL's `controlClient` is.
        let (_, body) = get_response(
            build_test_router(state.clone()),
            "/rest/koanCommand?to=dev-b&command=%7B%22type%22%3A%22pause%22%7D&u=mate&p=hunter22&v=1.16.1&c=test",
        )
        .await;
        assert!(body.contains("code=\"50\""), "{body}");
        let (_, body) = get_response(
            build_test_router(state.clone()),
            "/rest/getUser?username=mate&u=mate&p=hunter22&v=1.16.1&c=test",
        )
        .await;
        assert!(body.contains("shareRole=\"false\""), "{body}");
        let (_, body) = get_response(
            build_test_router(state),
            "/rest/getUser?username=owner&u=owner&p=sesame&v=1.16.1&c=test",
        )
        .await;
        assert!(body.contains("adminRole=\"true\""), "{body}");
    }

    /// Turn on `username`'s authenticator app, as the Account page would,
    /// and give back a function making its code for now.
    pub(crate) fn turn_on_totp(
        conn: &rusqlite::Connection,
        key: &[u8; 32],
        username: &str,
    ) -> impl Fn() -> String + use<> {
        let id = koan_core::db::queries::auth::get_user_by_username(conn, username)
            .unwrap()
            .unwrap()
            .id;
        let secret = koan_auth::second_factor::begin_totp(conn, key, id).unwrap();
        // Confirmed without spending this step's code, which the sign-in
        // under test spends.
        conn.execute("UPDATE totp SET confirmed = 1 WHERE user_id = ?1", [id])
            .unwrap();
        move || {
            let step = koan_auth::totp::step(koan_core::clock::unix_secs());
            format!("{:06}", koan_auth::totp::hotp(&secret, step as u64))
        }
    }

    #[tokio::test]
    async fn two_step_sign_in_refuses_the_password_alone_and_signs_in_with_a_code() {
        let (state, _dir) = test_state();
        let call = |path: String| {
            let state = state.clone();
            async move { get_response(build_test_router(state), &path).await.1 }
        };
        let enc = |p: &str| p.bytes().map(|b| format!("{b:02x}")).collect::<String>();
        let code = {
            let db = state.open_db().unwrap();
            turn_on_totp(&db.conn, state.totp_key.as_ref().unwrap(), "mate")
        };

        // The password alone signs in nowhere: not as `p`, not at koanSignIn.
        let body = call(format!(
            "/rest/ping?u=mate&p=enc:{}&v=1.16.1&c=test",
            enc("hunter22")
        ))
        .await;
        assert!(body.contains("code=\"45\""), "{body}");
        let sign_in = |code: &str| {
            format!(
                "/rest/koanSignIn?u=mate&p=enc:{}&code={code}&name=phone&v=1.16.1&c=test&f=json",
                enc("hunter22")
            )
        };
        let body = call(sign_in("")).await;
        assert!(body.contains("\"code\":45"), "{body}");
        let body = call(sign_in("000000")).await;
        assert!(body.contains("Wrong code"), "{body}");

        // With the code, a key; the same code again is spent.
        let now = code();
        let body = call(sign_in(&now)).await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let key = v["subsonic-response"]["join"]["apiKey"]
            .as_str()
            .unwrap()
            .to_owned();
        let body = call(format!("/rest/ping?apiKey={key}&v=1.16.1&c=test")).await;
        assert!(body.contains("status=\"ok\""), "{body}");
        let body = call(sign_in(&now)).await;
        assert!(body.contains("Wrong code"), "{body}");

        // A code is not a credential on any other request.
        let body = call(format!(
            "/rest/ping?u=mate&p=enc:{}&code={}&v=1.16.1&c=test",
            enc("hunter22"),
            code()
        ))
        .await;
        assert!(body.contains("code=\"45\""), "{body}");
    }

    /// The whole Subsonic app, throttle included, on a real socket that tells
    /// the throttle where each request came from.
    async fn serve_app(state: Arc<AppState>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = subsonic_app(state).into_make_service_with_connect_info::<std::net::SocketAddr>();
        tokio::spawn(async move { axum::serve(listener, app).await });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn passwords_sent_in_a_form_are_throttled_as_in_the_query() {
        let (state, _dir) = test_state();
        let base = serve_app(state).await;
        let http = reqwest::Client::new();
        let ping = format!("{base}/rest/ping");
        for _ in 0..10 {
            let body = post_text(&http, &ping, "u=mate&p=wrong&v=1.16.1&c=test").await;
            assert!(body.contains("code=\"40\""), "{body}");
        }
        let body = post_text(&http, &ping, "u=mate&p=hunter22&v=1.16.1&c=test").await;
        assert!(body.contains("Too many failed sign-ins"), "{body}");
        // In the query, the same account is as spent.
        let body = http
            .get(format!("{ping}?u=mate&p=hunter22&v=1.16.1&c=test"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(body.contains("Too many failed sign-ins"), "{body}");
    }

    #[tokio::test]
    async fn an_api_key_or_the_shared_token_in_a_form_is_not_a_password_guess() {
        let (state, _dir) = test_state();
        let key = api_key(&state, "mate");
        let base = serve_app(state).await;
        let http = reqwest::Client::new();
        let ping = format!("{base}/rest/ping");
        // More wrong keys than the address may fail passwords: none counts.
        for _ in 0..=AUTH_FAILURES_PER_ADDRESS_PER_MINUTE {
            let body = post_text(&http, &ping, "apiKey=wrong&v=1.16.1&c=test").await;
            assert!(body.contains("code=\"44\""), "{body}");
        }
        let body = post_text(&http, &ping, &format!("apiKey={key}&v=1.16.1&c=test")).await;
        assert!(body.contains("status=\"ok\""), "{body}");
        let body = post_text(&http, &ping, &auth_query("")).await;
        assert!(body.contains("status=\"ok\""), "{body}");
        let body = post_text(&http, &ping, "u=mate&p=hunter22&v=1.16.1&c=test").await;
        assert!(body.contains("status=\"ok\""), "{body}");
    }

    #[tokio::test]
    async fn test_api_key_auth() {
        let (state, _dir) = test_state();
        seed_data(&state);
        let owner = api_key(&state, "owner");
        let mate = api_key(&state, "mate");
        let app = || build_test_router(state.clone());

        let r = json_of(app(), &format!("/rest/ping?apiKey={owner}&f=json")).await;
        assert_envelope(&r, "ok");

        let r = json_of(app(), &format!("/rest/tokenInfo?apiKey={mate}&f=json")).await;
        assert_eq!(r["tokenInfo"]["username"], "mate");
        let r = json_of(app(), &format!("/rest/getUser?apiKey={mate}&f=json")).await;
        assert_eq!(r["user"]["username"], "mate");

        // `u` alongside a key, or any other credential, conflicts.
        for extra in ["u=owner", "p=sesame"] {
            let r = json_of(app(), &format!("/rest/ping?apiKey={owner}&{extra}&f=json")).await;
            assert_eq!(r["error"]["code"], 43, "{extra}");
            assert_eq!(r["error"]["helpUrl"], AUTH_HELP_URL);
        }

        let r = json_of(app(), "/rest/ping?apiKey=not-a-key&f=json").await;
        assert_eq!(r["error"]["code"], 44);

        // A readonly account's key is readonly.
        let r = json_of(app(), &format!("/rest/star?id=mf-1&apiKey={mate}&f=json")).await;
        assert_eq!(r["error"]["code"], 50);
        let r = json_of(app(), &format!("/rest/star?id=mf-1&apiKey={owner}&f=json")).await;
        assert_eq!(r["status"], "ok");

        // Accounts cannot use token auth, whatever the token: not even the
        // right one, after a password sign-in.
        let r = json_of(app(), "/rest/ping?u=mate&t=abc&s=def&f=json").await;
        assert_eq!(r["error"]["code"], 41);
        let r = json_of(app(), "/rest/ping?u=mate&p=hunter22&f=json").await;
        assert_eq!(r["status"], "ok");
        let right = format!("{:x}", md5::compute("hunter22salt"));
        let r = json_of(app(), &format!("/rest/ping?u=mate&t={right}&s=salt&f=json")).await;
        assert_eq!(r["error"]["code"], 41);
    }
}
