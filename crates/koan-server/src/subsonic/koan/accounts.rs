//! Accounts (admins only), API keys, app passwords, invites and sign-in.
//! An invite comes back as the account's username and a token, with the
//! password when one was just made; the app builds the link from the address
//! it already reaches this server at. `koanJoin` is the other end: an app
//! trading the token for a key.

use super::*;

pub(in crate::subsonic) fn respond_admin(
    state: &AppState,
    auth: &SubsonicParams,
    f: impl FnOnce(&Database, &Caller, XmlBuilder) -> Result<XmlBuilder, SubsonicError>,
) -> Response {
    let json = auth.wants_json();
    let result = validate_auth(auth, state).and_then(|caller| {
        if caller.role != Role::Admin {
            return Err(SubsonicError::not_authorized());
        }
        // Accounts are made, invited, promoted and deleted here: an admin's
        // app password must not turn into an account or a key of its own.
        caller.may_manage_credentials()?;
        let db = state.open_db()?;
        f(&db, &caller, SubsonicResponse::ok(json))
    });
    match result {
        Ok(builder) => builder.build(),
        Err(e) => SubsonicResponse::error(json, &e),
    }
}

pub(in crate::subsonic) fn account_error(e: koan_core::invite::AccountError) -> SubsonicError {
    use koan_core::invite::AccountError;
    match e {
        AccountError::NoSuchUser(_) => SubsonicError::not_found("User"),
        AccountError::Other(e) => SubsonicError::internal(e.to_string()),
        e => SubsonicError::new(SubsonicErrorCode::Generic, e.to_string()),
    }
}

pub(in crate::subsonic) fn role_param(params: &RawParams) -> Result<Role, SubsonicError> {
    params
        .get("role")
        .ok_or_else(|| SubsonicError::missing_param("role"))?
        .parse()
        .map_err(|_| SubsonicError::bad_param("role"))
}

pub(in crate::subsonic) fn username_param(params: &RawParams) -> Result<&str, SubsonicError> {
    params
        .get("username")
        .ok_or_else(|| SubsonicError::missing_param("username"))
}

/// The server's signing keypair: what invite tokens are signed and checked
/// with.
pub(in crate::subsonic) fn keypair() -> Result<crate::auth::Keypair, SubsonicError> {
    crate::auth::signing_keys().map_err(|e| SubsonicError::internal(e.to_string()))
}

/// An invite's node. `ttl` is the link's lifetime: a week for an admin's
/// invite, which is sent by email, and the QR code's for an account's own,
/// which is shown on a screen.
pub(in crate::subsonic) fn invite_node(
    conn: &rusqlite::Connection,
    user_id: i64,
    username: &str,
    password: Option<&str>,
    ttl: u64,
) -> Result<XmlNode, SubsonicError> {
    let token =
        koan_core::invite::mint_token(conn, &keypair()?.0, user_id, ttl).map_err(account_error)?;
    let qr_token = koan_core::invite::mint_token(
        conn,
        &keypair()?.0,
        user_id,
        koan_core::invite::QR_TOKEN_TTL_SECS,
    )
    .map_err(account_error)?;
    let node = XmlNode::new("invite")
        .attr("username", username)
        .attr("token", &token)
        .attr("qrToken", &qr_token);
    Ok(match password {
        Some(p) => node.attr("password", p),
        None => node,
    })
}

pub(in crate::subsonic) async fn koan_users(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        respond_admin(&state, &params.auth(), |db, _, b| {
            let users = queries::auth::list_users(&db.conn)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;
            Ok(b.child(XmlNode::new("users").list(
                "user",
                users.iter().map(|u| {
                    XmlNode::new("user")
                        .attr("username", &u.username)
                        .attr("role", u.role.as_str())
                }),
            )))
        })
    })
    .await
}

pub(in crate::subsonic) async fn koan_create_user(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        respond_admin(&state, &params.auth(), |db, _, b| {
            let username = username_param(&params)?;
            let role = role_param(&params)?;
            let made = koan_core::invite::create_account(&db.conn, username, role)
                .map_err(account_error)?;
            Ok(b.child(invite_node(
                &db.conn,
                made.id,
                username.trim(),
                Some(&made.password),
                koan_core::invite::TOKEN_TTL_SECS,
            )?))
        })
    })
    .await
}

/// An invite for an account. Any account may have its own, to sign in
/// another of its devices; an admin's may name any account, and with
/// `reset=true` give it a new password.
pub(in crate::subsonic) async fn koan_invite(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        respond_db_caller(&state, &params.auth(), Role::Readonly, |db, caller, b| {
            caller.may_manage_credentials()?;
            let username = username_param(&params)?.trim();
            let reset = params.get("reset") == Some("true");
            if caller.role != Role::Admin && (username != caller.username || reset) {
                return Err(SubsonicError::not_authorized());
            }
            let user = koan_core::invite::account(&db.conn, username).map_err(account_error)?;
            let password = if reset {
                let p = koan_core::invite::set_password(&db.conn, username, None)
                    .map_err(account_error)?;
                crate::clients::registry().disconnect(username);
                Some(p)
            } else {
                None
            };
            let ttl = if caller.role == Role::Admin {
                koan_core::invite::TOKEN_TTL_SECS
            } else {
                koan_core::invite::QR_TOKEN_TTL_SECS
            };
            Ok(b.child(invite_node(
                &db.conn,
                user.id,
                username,
                password.as_deref(),
                ttl,
            )?))
        })
    })
    .await
}

/// Set an account's password: an admin any account's, anyone else their own,
/// given the current one as `current`. The account's sessions, keys, app
/// passwords and links all end, as with any password change. Changing one's
/// own answers as `koanSignIn` does, with a new key named `name`: the key the
/// request came with is among those revoked.
///
/// Signed with an API key or the account's own password only: an app password
/// is a credential handed to one client, and the shared secret is no account's.
///
/// A wrong `current` counts against the account's sign-in budget, as a wrong
/// password at sign-in does, and a spent budget refuses the check from every
/// network, the account's own included: the sign-in throttle spares those, and
/// a check that is not a sign-in must not be repeatable from one without limit.
/// The request is usually signed with a key, which that throttle leaves alone,
/// so the budget is checked here.
///
/// POST only, so the passwords never sit in a URL, where a proxy in front of
/// the server would log them.
pub(in crate::subsonic) async fn koan_set_user_password(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        respond_db_caller(&state, &params.auth(), Role::Readonly, |db, caller, b| {
            caller.may_manage_credentials()?;
            let username = params
                .get("username")
                .map_or(caller.username.as_str(), str::trim);
            let own = username == caller.username;
            if caller.role != Role::Admin {
                if !own {
                    return Err(SubsonicError::new(
                        SubsonicErrorCode::NotAuthorized,
                        "only an admin can set another account's password",
                    ));
                }
                let current = params
                    .get("current")
                    .ok_or_else(|| SubsonicError::missing_param("current"))?;
                if state.users.exhausted(username) {
                    return Err(SubsonicError::new(
                        SubsonicErrorCode::Generic,
                        "Too many failed sign-ins for this account; try again in a minute",
                    ));
                }
                use crate::auth::password::Refused;
                match state.users.verify(username, current) {
                    Ok(_) => {}
                    Err(Refused::Busy) => return Err(SubsonicError::busy()),
                    Err(Refused::Wrong) => {
                        state.users.failed(username);
                        // Not error 40, which an app reads as its own sign-in
                        // failing.
                        return Err(SubsonicError::new(
                            SubsonicErrorCode::Generic,
                            "the current password is wrong",
                        ));
                    }
                }
            }
            let password = params
                .get("password")
                .ok_or_else(|| SubsonicError::missing_param("password"))?;
            koan_core::invite::set_password(&db.conn, username, Some(password))
                .map_err(account_error)?;
            crate::clients::registry().disconnect(username);
            if !own {
                return Ok(b);
            }
            let name = koan_core::invite::device_name(params.get("name").unwrap_or_default());
            let (_, api_key) = queries::api_keys::replace_api_key(&db.conn, caller.user_id, &name)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;
            Ok(b.child(
                XmlNode::new("join")
                    .attr("username", &caller.username)
                    .attr("apiKey", &api_key),
            ))
        })
    })
    .await
}

/// Where an assistant connects to this server (`/mcp`), and the page that says
/// how (`/connect`). Signing in is MCP's own OAuth, through this server's web
/// pages; this only says where.
pub(in crate::subsonic) async fn koan_mcp(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    offload_response(move || {
        respond_db_caller(&state, &params, Role::Readonly, |_, _, b| {
            // An app's own server mounts no MCP.
            let base = state
                .server_url()
                .filter(|_| state.host_key.is_none())
                .ok_or_else(|| SubsonicError::not_found("An address for assistants"))?;
            Ok(b.child(
                XmlNode::new("mcp")
                    .attr("url", &format!("{base}/mcp"))
                    .attr("connect", &format!("{base}/connect")),
            ))
        })
    })
    .await
}

/// The caller's API keys, with the one the request signed in with marked
/// `current`. Never the keys: only their hashes are kept.
pub(in crate::subsonic) async fn koan_api_keys(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let auth = params.auth();
        respond_db_caller(&state, &auth, Role::Readonly, |db, caller, b| {
            caller.may_manage_credentials()?;
            let internal = |e: rusqlite::Error| SubsonicError::internal(e.to_string());
            let keys = queries::api_keys::list_api_keys(&db.conn, Some(caller.user_id))
                .map_err(internal)?;
            let current = match auth.api_key.as_deref() {
                Some(key) => queries::api_keys::id_of(&db.conn, key).map_err(internal)?,
                None => None,
            };
            Ok(b.child(XmlNode::new("apiKeys").list(
                "apiKey",
                keys.iter().map(|k| api_key_node(k, current == Some(k.id))),
            )))
        })
    })
    .await
}

pub(in crate::subsonic) fn api_key_node(
    key: &queries::api_keys::ApiKeyRow,
    current: bool,
) -> XmlNode {
    XmlNode::new("apiKey")
        .attr_int("id", key.id)
        .attr("name", &key.name)
        .attr("created", &iso(key.created_at))
        .attr_opt("lastUsed", key.last_used_at.map(iso).as_deref())
        .attr_bool("current", current)
}

/// Make an API key named `name` for the caller, for another Subsonic app. The
/// answer carries the key, once.
pub(in crate::subsonic) async fn koan_create_api_key(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        respond_db_caller(&state, &params.auth(), Role::Readonly, |db, caller, b| {
            caller.may_manage_credentials()?;
            let name = params
                .get("name")
                .map(koan_core::invite::device_name)
                .ok_or_else(|| SubsonicError::missing_param("name"))?;
            let (id, key) = queries::api_keys::create_api_key(&db.conn, caller.user_id, &name)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;
            Ok(b.child(
                XmlNode::new("apiKey")
                    .attr_int("id", id)
                    .attr("name", &name)
                    .attr("key", &key),
            ))
        })
    })
    .await
}

/// Revoke one of the caller's API keys, by `id`. Not the one the request is
/// signed in with: that is signing out, which the app does itself.
pub(in crate::subsonic) async fn koan_revoke_api_key(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let auth = params.auth();
        respond_db_caller(&state, &auth, Role::Readonly, |db, caller, b| {
            caller.may_manage_credentials()?;
            let internal = |e: rusqlite::Error| SubsonicError::internal(e.to_string());
            let id: i64 = params
                .get("id")
                .ok_or_else(|| SubsonicError::missing_param("id"))?
                .parse()
                .map_err(|_| SubsonicError::bad_param("id"))?;
            if let Some(key) = auth.api_key.as_deref()
                && queries::api_keys::id_of(&db.conn, key).map_err(internal)? == Some(id)
            {
                return Err(SubsonicError::new(
                    SubsonicErrorCode::Generic,
                    "this device signs in with that key; sign out instead",
                ));
            }
            if !queries::api_keys::revoke_api_key(&db.conn, id, Some(caller.user_id))
                .map_err(internal)?
            {
                return Err(SubsonicError::not_found("API key"));
            }
            Ok(b)
        })
    })
    .await
}

/// The key app passwords are sealed under, which a server without a signing
/// key does not have; it then offers none.
pub(in crate::subsonic) fn app_password_key(state: &AppState) -> Result<&[u8; 32], SubsonicError> {
    state
        .app_key
        .as_ref()
        .ok_or_else(|| SubsonicError::not_found("App passwords"))
}

/// The caller's app passwords. Never the passwords themselves.
pub(in crate::subsonic) async fn koan_app_passwords(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        respond_db_caller(&state, &params.auth(), Role::Readonly, |db, caller, b| {
            caller.may_manage_credentials()?;
            app_password_key(&state)?;
            let passwords = queries::app_passwords::list_app_passwords(&db.conn, caller.user_id)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;
            Ok(b.child(
                XmlNode::new("appPasswords")
                    .list("appPassword", passwords.iter().map(app_password_node)),
            ))
        })
    })
    .await
}

pub(in crate::subsonic) fn app_password_node(
    row: &queries::app_passwords::AppPasswordRow,
) -> XmlNode {
    XmlNode::new("appPassword")
        .attr_int("id", row.id)
        .attr("name", &row.name)
        .attr("created", &iso(row.created_at))
        .attr_opt("lastUsed", row.last_used_at.map(iso).as_deref())
}

/// Make an app password named `name` for the caller, as the web UI's Account
/// page does. The answer carries the password, once.
pub(in crate::subsonic) async fn koan_create_app_password(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        respond_db_caller(&state, &params.auth(), Role::Readonly, |db, caller, b| {
            caller.may_manage_credentials()?;
            let key = app_password_key(&state)?;
            let name = params
                .get("name")
                .ok_or_else(|| SubsonicError::missing_param("name"))?;
            let name = queries::app_passwords::app_password_name(name)
                .ok_or_else(|| SubsonicError::bad_param("name"))?;
            use queries::app_passwords::CreateAppPasswordError;
            let (id, password) =
                queries::app_passwords::create_app_password(&db.conn, key, caller.user_id, &name)
                    .map_err(|e| match e {
                    CreateAppPasswordError::TooMany => {
                        SubsonicError::new(SubsonicErrorCode::Generic, e.to_string())
                    }
                    CreateAppPasswordError::Db(e) => SubsonicError::internal(e.to_string()),
                })?;
            Ok(b.child(
                XmlNode::new("appPassword")
                    .attr_int("id", id)
                    .attr("name", &name)
                    .attr("password", &password),
            ))
        })
    })
    .await
}

/// Revoke one of the caller's app passwords, by `id`.
pub(in crate::subsonic) async fn koan_revoke_app_password(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        respond_db_caller(&state, &params.auth(), Role::Readonly, |db, caller, b| {
            caller.may_manage_credentials()?;
            app_password_key(&state)?;
            let id: i64 = params
                .get("id")
                .ok_or_else(|| SubsonicError::missing_param("id"))?
                .parse()
                .map_err(|_| SubsonicError::bad_param("id"))?;
            if !queries::app_passwords::revoke_app_password(&db.conn, id, caller.user_id)
                .map_err(|e| SubsonicError::internal(e.to_string()))?
            {
                return Err(SubsonicError::not_found("App password"));
            }
            Ok(b)
        })
    })
    .await
}

/// Revoke the API key the request is signed in with: for an app giving up a
/// key it no longer holds, such as the one a join replaced.
pub(in crate::subsonic) async fn koan_revoke_key(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let auth = params.auth();
        respond_db_caller(&state, &auth, Role::Readonly, |db, _, b| {
            let key = auth.api_key.as_deref().ok_or_else(|| {
                SubsonicError::new(SubsonicErrorCode::Generic, "sign in with the key to revoke")
            })?;
            queries::api_keys::revoke_api_key_value(&db.conn, key)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;
            Ok(b)
        })
    })
    .await
}

/// Trade the account's password, proved by this request, for an API key
/// named `name`: what a koan app does once on signing in with a password, so
/// it keeps a key of the device's own and never sends the password again.
/// Answers as `koanJoin` does. Only the account's own password will do: a key
/// or app password is already a credential of its own, and the shared secret
/// is no account's. The key replaces any of the account's with the same name,
/// so a device that signs in again holds the one key.
pub(in crate::subsonic) async fn koan_sign_in(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let mut auth = params.auth();
        // Present, if empty, so that error 45 asks for a code rather than a key.
        auth.code = Some(params.get("code").unwrap_or_default().to_owned());
        respond_db_caller(&state, &auth, Role::Readonly, |db, caller, b| {
            if caller.via != Via::Password {
                return Err(SubsonicError::new(
                    SubsonicErrorCode::NotAuthorized,
                    "sign in with the account's own password to be given a key",
                ));
            }
            let name = koan_core::invite::device_name(params.get("name").unwrap_or_default());
            // On a demo every visitor is the one account, and devices share
            // names ("iPhone"): replacing would sign the last visitor out.
            let made = if crate::demo::enabled() {
                queries::api_keys::create_api_key(&db.conn, caller.user_id, &name)
            } else {
                queries::api_keys::replace_api_key(&db.conn, caller.user_id, &name)
            };
            let (_, api_key) = made.map_err(|e| SubsonicError::internal(e.to_string()))?;
            Ok(b.child(
                XmlNode::new("join")
                    .attr("username", &caller.username)
                    .attr("apiKey", &api_key),
            ))
        })
    })
    .await
}

/// Trade an invite token for an API key named `name`. The token is the
/// credential, so this alone of koan's endpoints asks for no other.
pub(in crate::subsonic) async fn koan_join(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let json = params.auth().wants_json();
        let joined = params
            .get("invite")
            .ok_or_else(|| SubsonicError::missing_param("invite"))
            .and_then(|token| {
                let public = &keypair()?.1;
                let db = state.open_db()?;
                koan_core::invite::redeem(
                    &db.conn,
                    public,
                    token,
                    params.get("name").unwrap_or_default(),
                )
                .map_err(account_error)
            });
        match joined {
            Ok(j) => SubsonicResponse::ok(json)
                .child(
                    XmlNode::new("join")
                        .attr("username", &j.username)
                        .attr("apiKey", &j.api_key),
                )
                .build(),
            Err(e) => SubsonicResponse::error(json, &e),
        }
    })
    .await
}

pub(in crate::subsonic) async fn koan_set_user_role(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        respond_admin(&state, &params.auth(), |db, _, b| {
            koan_core::invite::set_role(&db.conn, username_param(&params)?, role_param(&params)?)
                .map_err(account_error)?;
            Ok(b)
        })
    })
    .await
}

pub(in crate::subsonic) async fn koan_delete_user(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        respond_admin(&state, &params.auth(), |db, caller, b| {
            let username = username_param(&params)?;
            if username == caller.username {
                return Err(SubsonicError::new(
                    SubsonicErrorCode::Generic,
                    "an account cannot delete itself",
                ));
            }
            koan_core::invite::delete_account(&db.conn, username).map_err(account_error)?;
            crate::clients::registry().disconnect(username);
            Ok(b)
        })
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subsonic::testing::*;

    #[tokio::test]
    async fn a_password_sign_in_is_traded_for_a_key_of_the_accounts_own() {
        let (state, _dir) = test_state();
        let call = |path: String| {
            let state = state.clone();
            async move { get_response(build_test_router(state), &path).await.1 }
        };
        let enc = |p: &str| p.bytes().map(|b| format!("{b:02x}")).collect::<String>();

        let body = call("/rest/getOpenSubsonicExtensions?f=json".to_owned()).await;
        assert!(
            body.contains("koanSignIn"),
            "listed for clients to find: {body}"
        );

        let body = call(format!(
            "/rest/koanSignIn?u=mate&p=enc:{}&name=Mate%27s%20iPhone&v=1.16.1&c=test&f=json",
            enc("hunter22")
        ))
        .await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["subsonic-response"]["join"]["username"], "mate", "{body}");
        let key = v["subsonic-response"]["join"]["apiKey"]
            .as_str()
            .unwrap()
            .to_owned();
        let body = call(format!("/rest/ping?apiKey={key}&v=1.16.1&c=test")).await;
        assert!(body.contains("status=\"ok\""), "{body}");
        let db = state.open_db().unwrap();
        let mate = koan_core::db::queries::auth::get_user_by_username(&db.conn, "mate")
            .unwrap()
            .unwrap();
        let keys = queries::api_keys::list_api_keys(&db.conn, Some(mate.id)).unwrap();
        assert_eq!(
            keys.iter().map(|k| k.name.as_str()).collect::<Vec<_>>(),
            ["Mate's iPhone"],
            "the account's own key, named for the device"
        );

        // Signing in again from the same device replaces its key.
        let body = call(format!(
            "/rest/koanSignIn?u=mate&p=enc:{}&name=Mate%27s%20iPhone&v=1.16.1&c=test&f=json",
            enc("hunter22")
        ))
        .await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let again = v["subsonic-response"]["join"]["apiKey"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_ne!(again, key);
        let body = call(format!("/rest/ping?apiKey={key}&v=1.16.1&c=test")).await;
        assert!(body.contains("code=\"44\""), "the old key is gone: {body}");
        let keys = queries::api_keys::list_api_keys(&db.conn, Some(mate.id)).unwrap();
        assert_eq!(keys.len(), 1, "one key per device name");
        let key = again;

        // Only the account's own password: not a key it already holds, not an
        // app password, not the shared secret, which is no account's, and not
        // a wrong password.
        let body = call(format!(
            "/rest/koanSignIn?apiKey={key}&name=more&v=1.16.1&c=test"
        ))
        .await;
        assert!(body.contains("code=\"50\""), "{body}");
        let app_password = queries::app_passwords::create_app_password(
            &db.conn,
            state.app_key.as_ref().unwrap(),
            mate.id,
            "arpeggi",
        )
        .unwrap()
        .1;
        let body = call(format!(
            "/rest/koanSignIn?u=mate&p=enc:{}&name=more&v=1.16.1&c=test",
            enc(&app_password)
        ))
        .await;
        assert!(body.contains("code=\"50\""), "{body}");
        let body =
            call("/rest/koanSignIn?u=testuser&p=testpass&name=shared&v=1.16.1&c=test".to_owned())
                .await;
        assert!(body.contains("code=\"50\""), "{body}");
        let body = call(format!(
            "/rest/koanSignIn?u=mate&p=enc:{}&name=guess&v=1.16.1&c=test",
            enc("wrong")
        ))
        .await;
        assert!(body.contains("code=\"40\""), "{body}");
        assert_eq!(
            queries::api_keys::list_api_keys(&db.conn, None)
                .unwrap()
                .len(),
            1,
            "no key made for any of them"
        );
    }

    /// An app password is one client's credential and the shared secret is no
    /// account's: neither may make an account, an invite or a key, change who
    /// may do what, or sign a device in.
    #[tokio::test]
    async fn app_passwords_and_the_shared_secret_manage_no_credentials() {
        let (state, _dir) = test_state();
        let db = state.open_db().unwrap();
        let app_password_for = |username: &str| {
            let user = queries::auth::get_user_by_username(&db.conn, username)
                .unwrap()
                .unwrap();
            queries::app_passwords::create_app_password(
                &db.conn,
                state.app_key.as_ref().unwrap(),
                user.id,
                "arpeggi",
            )
            .unwrap()
            .1
        };
        let as_app_password = |username: &str| {
            let password = app_password_for(username);
            let salt = "c0ffee";
            let token = format!("{:x}", md5::compute(format!("{password}{salt}")));
            [
                format!("u={username}&p={password}"),
                format!("u={username}&t={token}&s={salt}"),
            ]
        };
        let call = |path: String| {
            let state = state.clone();
            async move { get_response(build_test_router(state), &path).await.1 }
        };

        let mut admin = as_app_password("owner").to_vec();
        admin.push(auth_query(""));
        for auth in &admin {
            for path in [
                "koanUsers?",
                "koanCreateUser?username=mallory&role=admin&",
                "koanInvite?username=owner&",
                "koanInvite?username=owner&reset=true&",
                "koanSetUserRole?username=mate&role=admin&",
                "koanDeleteUser?username=mate&",
            ] {
                let body = call(format!("/rest/{path}{auth}&v=1.16.1&c=test")).await;
                assert!(body.contains("code=\"50\""), "{path} as {auth}: {body}");
            }
        }

        let opened = crate::pair::pairings()
            .open("Den TV", Some("0192-den"), "198.51.100.7".parse().unwrap())
            .unwrap();
        let mut mate = as_app_password("mate").to_vec();
        mate.push(auth_query(""));
        for auth in &mate {
            for path in [
                "koanPairApprove?",
                "koanPairApprove?decline=true&",
                "koanPairInfo?",
            ] {
                let body = call(format!(
                    "/rest/{path}pair={}&{auth}&v=1.16.1&c=test",
                    opened.id
                ))
                .await;
                assert!(body.contains("code=\"50\""), "{path} as {auth}: {body}");
            }
        }

        assert!(
            queries::auth::get_user_by_username(&db.conn, "mallory")
                .unwrap()
                .is_none()
        );
        assert_eq!(
            queries::auth::get_user_by_username(&db.conn, "mate")
                .unwrap()
                .unwrap()
                .role,
            Role::Readonly
        );
        assert!(
            queries::api_keys::list_api_keys(&db.conn, None)
                .unwrap()
                .is_empty(),
            "no key made"
        );
        // Still waiting for someone who may approve it.
        let id = opened.id.clone();
        let device = crate::pair::device(opened, true);
        let body = call(format!(
            "/rest/koanPairApprove?pair={id}&u=mate&p=hunter22&v=1.16.1&c=test"
        ))
        .await;
        assert!(body.contains("Den TV"), "{body}");
        device.join().unwrap();
    }

    #[tokio::test]
    async fn passwords_are_set_by_admins_and_changed_by_their_owners() {
        let (state, _dir) = test_state();
        let set = |form: String| {
            let state = state.clone();
            async move {
                let body =
                    post_form(build_test_router(state), "/rest/koanSetUserPassword", &form).await;
                serde_json::from_str::<serde_json::Value>(&body).unwrap()["subsonic-response"]
                    .clone()
            }
        };
        let ping = |query: String| {
            let state = state.clone();
            async move {
                get_response(
                    build_test_router(state),
                    &format!("/rest/ping?{query}&v=1.16.1&c=test"),
                )
                .await
                .1
            }
        };
        let db = state.open_db().unwrap();
        let mate = queries::auth::get_user_by_username(&db.conn, "mate")
            .unwrap()
            .unwrap();
        let (_, key) = queries::api_keys::replace_api_key(&db.conn, mate.id, "phone").unwrap();
        let as_mate = format!("apiKey={key}&v=1.16.1&c=test&f=json");

        // POST only: the passwords never sit in a URL.
        let (status, _) = get_response(
            build_test_router(state.clone()),
            &format!(
                "/rest/koanSetUserPassword?current=hunter22&password=correct%20horse&{as_mate}"
            ),
        )
        .await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);

        // Only their own, and only with the current password.
        let v = set(format!("username=owner&password=taken%20over&{as_mate}")).await;
        assert_eq!(v["error"]["code"], 50, "{v}");
        let v = set(format!("password=new%20enough&{as_mate}")).await;
        assert_eq!(v["error"]["code"], 10, "{v}");
        let v = set(format!("current=wrong&password=new%20enough&{as_mate}")).await;
        assert_eq!(
            v["error"]["code"], 0,
            "not 40, which reads as the app signed out: {v}"
        );
        assert!(
            ping(format!("apiKey={key}"))
                .await
                .contains("status=\"ok\"")
        );

        // Changed: every old credential ends, and the device gets a new key.
        let v = set(format!(
            "current=hunter22&password=correct%20horse&name=phone&{as_mate}"
        ))
        .await;
        assert_eq!(v["join"]["username"], "mate", "{v}");
        let fresh = v["join"]["apiKey"].as_str().unwrap().to_owned();
        assert!(ping(format!("apiKey={key}")).await.contains("code=\"44\""));
        assert!(
            ping(format!("apiKey={fresh}"))
                .await
                .contains("status=\"ok\"")
        );
        assert!(
            ping("u=mate&p=hunter22".into())
                .await
                .contains("code=\"40\"")
        );
        assert!(
            ping("u=mate&p=correct%20horse".into())
                .await
                .contains("status=\"ok\"")
        );

        // An admin sets anyone's, without theirs, and says nothing back.
        let owner = "u=owner&p=sesame&v=1.16.1&c=test&f=json";
        let v = set(format!("username=mate&password=battery%20staple&{owner}")).await;
        assert_eq!(v["status"], "ok", "{v}");
        assert!(v.get("join").is_none(), "{v}");
        assert!(
            ping(format!("apiKey={fresh}"))
                .await
                .contains("code=\"44\"")
        );
        assert!(
            ping("u=mate&p=battery%20staple".into())
                .await
                .contains("status=\"ok\"")
        );
        let v = set(format!("username=mate&password=short&{owner}")).await;
        assert_eq!(v["error"]["code"], 0, "too short: {v}");
    }

    /// An app password is one client's credential and the shared secret is no
    /// account's: neither may set a password, nor try current passwords.
    #[tokio::test]
    async fn only_a_key_or_the_password_itself_may_set_a_password() {
        let (state, _dir) = test_state();
        let set = |auth: String| {
            let state = state.clone();
            async move {
                post_form(
                    build_test_router(state),
                    "/rest/koanSetUserPassword",
                    &format!("current=hunter22&password=correct%20horse&{auth}&v=1.16.1&c=test"),
                )
                .await
            }
        };
        let db = state.open_db().unwrap();
        let mate = queries::auth::get_user_by_username(&db.conn, "mate")
            .unwrap()
            .unwrap();
        let app_password = queries::app_passwords::create_app_password(
            &db.conn,
            state.app_key.as_ref().unwrap(),
            mate.id,
            "arpeggi",
        )
        .unwrap()
        .1;
        let salt = "c0ffee";
        let token = format!("{:x}", md5::compute(format!("{app_password}{salt}")));

        let body = set(format!("u=mate&p={app_password}")).await;
        assert!(body.contains("code=\"50\""), "app password as p: {body}");
        let body = set(format!("u=mate&t={token}&s={salt}")).await;
        assert!(
            body.contains("code=\"50\""),
            "app password as a token: {body}"
        );
        let body = set(auth_query("")).await;
        assert!(body.contains("code=\"50\""), "the shared secret: {body}");
        assert!(
            queries::auth::get_user_by_username(&db.conn, "mate")
                .unwrap()
                .is_some_and(
                    |u| koan_core::auth::verify_password("hunter22", &u.password_hash).is_ok()
                ),
            "unchanged"
        );
    }

    /// Checked in the handler, from every network: the request is signed with
    /// a key, which the sign-in throttle leaves alone, and that throttle spares
    /// networks the account signed in from, which must not make guessing here
    /// free.
    #[tokio::test]
    async fn a_wrong_current_password_spends_the_sign_in_budget() {
        let (state, _dir) = test_state();
        let db = state.open_db().unwrap();
        let mate = queries::auth::get_user_by_username(&db.conn, "mate")
            .unwrap()
            .unwrap();
        let (_, key) = queries::api_keys::replace_api_key(&db.conn, mate.id, "phone").unwrap();
        let change = |current: &str| {
            let state = state.clone();
            let form =
                format!("current={current}&password=another%20one&apiKey={key}&v=1.16.1&c=test");
            async move { post_form(build_test_router(state), "/rest/koanSetUserPassword", &form).await }
        };
        // Signing in by password makes this network one the account is known on.
        let app = subsonic_app(state.clone());
        let body = get_response(app, "/rest/ping?u=mate&p=hunter22&v=1.16.1&c=test")
            .await
            .1;
        assert!(body.contains("status=\"ok\""), "{body}");

        let body = change("wrong").await;
        assert!(body.contains("the current password is wrong"), "{body}");
        for _ in 1..crate::auth::password::FAILURES_PER_USERNAME_PER_MINUTE {
            state.users.failed("mate");
        }
        let body = change("hunter22").await;
        assert!(body.contains("Too many failed sign-ins"), "{body}");
        let body = get_response(
            build_test_router(state.clone()),
            &format!("/rest/ping?apiKey={key}&v=1.16.1&c=test"),
        )
        .await
        .1;
        assert!(
            body.contains("status=\"ok\""),
            "the key still works: {body}"
        );
    }

    #[tokio::test]
    async fn an_account_lists_makes_and_revokes_its_own_api_keys() {
        let (state, _dir) = test_state();
        let phone = api_key(&state, "mate");
        let owners = api_key(&state, "owner");
        let auth = format!("apiKey={phone}&v=1.16.1&c=test&f=json");
        let json = |body: String| -> serde_json::Value {
            serde_json::from_str::<serde_json::Value>(&body).unwrap()["subsonic-response"].clone()
        };
        let post = |path: &str, form: String| {
            let state = state.clone();
            let path = path.to_owned();
            async move { json(post_form(build_test_router(state), &path, &form).await) }
        };
        let list = || {
            let state = state.clone();
            let auth = auth.clone();
            async move {
                json(
                    get_response(
                        build_test_router(state),
                        &format!("/rest/koanApiKeys?{auth}"),
                    )
                    .await
                    .1,
                )
            }
        };

        let made = post("/rest/koanCreateApiKey", format!("name=Feishin&{auth}")).await;
        let key = made["apiKey"]["key"].as_str().unwrap().to_owned();
        let id = made["apiKey"]["id"].as_i64().unwrap();
        assert_eq!(made["apiKey"]["name"], "Feishin", "{made}");
        let pinged = get_response(
            build_test_router(state.clone()),
            &format!("/rest/ping?apiKey={key}&v=1.16.1&c=test"),
        )
        .await
        .1;
        assert!(pinged.contains("status=\"ok\""), "{pinged}");

        // Only this account's, the request's own marked, and never a key.
        let v = list().await;
        let keys = v["apiKeys"]["apiKey"].as_array().unwrap();
        assert_eq!(keys.len(), 2, "{v}");
        assert_eq!(keys[0]["name"], "test");
        assert_eq!(keys[0]["current"], true);
        assert_eq!(keys[1]["current"], false);
        assert!(keys[0]["created"].as_str().unwrap().ends_with('Z'), "{v}");
        assert!(
            !v.to_string().contains(&phone) && !v.to_string().contains(&key),
            "{v}"
        );

        // Making and revoking take POST.
        let (status, _) = get_response(
            build_test_router(state.clone()),
            &format!("/rest/koanCreateApiKey?name=x&{auth}"),
        )
        .await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);

        // Not this device's own key, and not another account's.
        let db = state.open_db().unwrap();
        let own = queries::api_keys::id_of(&db.conn, &phone).unwrap().unwrap();
        let v = post("/rest/koanRevokeApiKey", format!("id={own}&{auth}")).await;
        assert!(
            v["error"]["message"].as_str().unwrap().contains("sign out"),
            "{v}"
        );
        let theirs = queries::api_keys::id_of(&db.conn, &owners)
            .unwrap()
            .unwrap();
        let v = post("/rest/koanRevokeApiKey", format!("id={theirs}&{auth}")).await;
        assert_eq!(v["error"]["code"], 70, "{v}");

        let v = post("/rest/koanRevokeApiKey", format!("id={id}&{auth}")).await;
        assert_eq!(v["status"], "ok", "{v}");
        let pinged = get_response(
            build_test_router(state.clone()),
            &format!("/rest/ping?apiKey={key}&v=1.16.1&c=test"),
        )
        .await
        .1;
        assert!(pinged.contains("code=\"44\""), "{pinged}");
        assert_eq!(
            list().await["apiKeys"]["apiKey"].as_array().unwrap().len(),
            1
        );
    }

    #[tokio::test]
    async fn an_account_lists_makes_and_revokes_its_own_app_passwords() {
        let (state, _dir) = test_state();
        let phone = api_key(&state, "mate");
        let auth = format!("apiKey={phone}&v=1.16.1&c=test&f=json");
        let json = |body: String| -> serde_json::Value {
            serde_json::from_str::<serde_json::Value>(&body).unwrap()["subsonic-response"].clone()
        };
        let post = |path: &str, form: String| {
            let state = state.clone();
            let path = path.to_owned();
            async move { json(post_form(build_test_router(state), &path, &form).await) }
        };
        let ping = |query: String| {
            let state = state.clone();
            async move {
                get_response(
                    build_test_router(state),
                    &format!("/rest/ping?{query}&v=1.16.1&c=test"),
                )
                .await
                .1
            }
        };

        let extensions = get_response(
            build_test_router(state.clone()),
            "/rest/getOpenSubsonicExtensions?f=json",
        )
        .await
        .1;
        assert!(extensions.contains("koanAppPasswords"), "{extensions}");

        let made = post(
            "/rest/koanCreateAppPassword",
            format!("name=Arpeggi&{auth}"),
        )
        .await;
        let password = made["appPassword"]["password"].as_str().unwrap().to_owned();
        let id = made["appPassword"]["id"].as_i64().unwrap();
        assert_eq!(made["appPassword"]["name"], "Arpeggi", "{made}");
        let salt = "c0ffee";
        let token = format!("{:x}", md5::compute(format!("{password}{salt}")));
        assert!(
            ping(format!("u=mate&t={token}&s={salt}"))
                .await
                .contains("status=\"ok\"")
        );

        // Listed without the password; a name is required.
        let v = post("/rest/koanAppPasswords", auth.clone()).await;
        let listed = v["appPasswords"]["appPassword"].as_array().unwrap();
        assert_eq!(listed.len(), 1, "{v}");
        assert_eq!(listed[0]["id"], id);
        assert!(listed[0]["created"].as_str().unwrap().ends_with('Z'), "{v}");
        assert!(!v.to_string().contains(&password), "{v}");
        let v = post("/rest/koanCreateAppPassword", format!("name=%20&{auth}")).await;
        assert_eq!(v["error"]["code"], 10, "{v}");

        // Another account's is neither listed nor revocable.
        let db = state.open_db().unwrap();
        let owner = queries::auth::get_user_by_username(&db.conn, "owner")
            .unwrap()
            .unwrap();
        let (theirs, _) = queries::app_passwords::create_app_password(
            &db.conn,
            state.app_key.as_ref().unwrap(),
            owner.id,
            "theirs",
        )
        .unwrap();
        let v = post("/rest/koanAppPasswords", auth.clone()).await;
        assert_eq!(
            v["appPasswords"]["appPassword"].as_array().unwrap().len(),
            1,
            "{v}"
        );
        let v = post("/rest/koanRevokeAppPassword", format!("id={theirs}&{auth}")).await;
        assert_eq!(v["error"]["code"], 70, "{v}");
        assert_eq!(
            queries::app_passwords::list_app_passwords(&db.conn, owner.id)
                .unwrap()
                .len(),
            1
        );

        // An app password manages none, its own included.
        let as_app_password = format!("u=mate&p={password}&v=1.16.1&c=test&f=json");
        for (path, form) in [
            ("/rest/koanAppPasswords", as_app_password.clone()),
            (
                "/rest/koanCreateAppPassword",
                format!("name=x&{as_app_password}"),
            ),
            (
                "/rest/koanRevokeAppPassword",
                format!("id={id}&{as_app_password}"),
            ),
        ] {
            let v = post(path, form).await;
            assert_eq!(v["error"]["code"], 50, "{path}: {v}");
        }

        let v = post("/rest/koanRevokeAppPassword", format!("id={id}&{auth}")).await;
        assert_eq!(v["status"], "ok", "{v}");
        assert!(
            !ping(format!("u=mate&t={token}&s={salt}"))
                .await
                .contains("status=\"ok\"")
        );

        // At most so many, refused with a reason.
        for n in 0..queries::app_passwords::MAX_APP_PASSWORDS {
            let v = post("/rest/koanCreateAppPassword", format!("name=app{n}&{auth}")).await;
            assert_eq!(v["status"], "ok", "{v}");
        }
        let v = post(
            "/rest/koanCreateAppPassword",
            format!("name=one%20more&{auth}"),
        )
        .await;
        assert_eq!(v["error"]["code"], 0, "{v}");
        assert!(
            v["error"]["message"].as_str().unwrap().contains("at most"),
            "{v}"
        );
    }

    /// An app password must not mint keys that outlive it, nor revoke the
    /// account's devices; the shared secret is no account's.
    #[tokio::test]
    async fn only_a_key_or_the_password_itself_may_manage_api_keys() {
        let (state, _dir) = test_state();
        let db = state.open_db().unwrap();
        let mate = queries::auth::get_user_by_username(&db.conn, "mate")
            .unwrap()
            .unwrap();
        let (phone, _) = queries::api_keys::replace_api_key(&db.conn, mate.id, "phone").unwrap();
        let app_password = queries::app_passwords::create_app_password(
            &db.conn,
            state.app_key.as_ref().unwrap(),
            mate.id,
            "arpeggi",
        )
        .unwrap()
        .1;
        let salt = "c0ffee";
        let token = format!("{:x}", md5::compute(format!("{app_password}{salt}")));
        let callers = [
            ("app password as p", format!("u=mate&p={app_password}")),
            (
                "app password as a token",
                format!("u=mate&t={token}&s={salt}"),
            ),
            ("the shared secret", auth_query("")),
        ];
        for (who, auth) in &callers {
            let auth = format!("{auth}&v=1.16.1&c=test");
            for (path, form) in [
                ("/rest/koanCreateApiKey", format!("name=x&{auth}")),
                ("/rest/koanRevokeApiKey", format!("id={phone}&{auth}")),
                ("/rest/koanApiKeys", auth.clone()),
            ] {
                let body = post_form(build_test_router(state.clone()), path, &form).await;
                assert!(body.contains("code=\"50\""), "{who} on {path}: {body}");
            }
        }
        let keys = queries::api_keys::list_api_keys(&db.conn, None).unwrap();
        assert_eq!(keys.len(), 1, "none made, none revoked");
        assert_eq!(keys[0].id, phone);
    }

    #[tokio::test]
    async fn admins_manage_accounts_through_the_koan_endpoints() {
        let (state, _dir) = test_state();
        let call = |path: String| {
            let state = state.clone();
            async move { get_response(build_test_router(state), &path).await.1 }
        };
        let owner = "u=owner&p=sesame&v=1.16.1&c=test&f=json";

        let body = call("/rest/koanUsers?u=mate&p=hunter22&v=1.16.1&c=test".to_owned()).await;
        assert!(body.contains("code=\"50\""), "{body}");

        let body = call(format!(
            "/rest/koanCreateUser?username=sarita&role=readonly&{owner}"
        ))
        .await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let invite = &v["subsonic-response"]["invite"];
        let password = invite["password"].as_str().unwrap().to_owned();
        let token = invite["token"].as_str().unwrap().to_owned();
        let qr_token = invite["qrToken"].as_str().unwrap().to_owned();
        assert_ne!(qr_token, token);
        assert_eq!(invite["username"], "sarita");

        // The password works in any Subsonic app; the token, redeemed with
        // nothing else, makes this device a key.
        let body = call(format!("/rest/ping?u=sarita&p={password}&v=1.16.1&c=test")).await;
        assert!(body.contains("status=\"ok\""), "{body}");
        let body = call(format!(
            "/rest/koanJoin?invite={token}&name=Sarita%27s%20iPhone&f=json"
        ))
        .await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            v["subsonic-response"]["join"]["username"], "sarita",
            "{body}"
        );
        let key = v["subsonic-response"]["join"]["apiKey"].as_str().unwrap();
        let body = call(format!("/rest/ping?apiKey={key}&v=1.16.1&c=test")).await;
        assert!(body.contains("status=\"ok\""), "{body}");
        // Any account has an invite of its own, to link another device, and
        // only its own.
        let sarita = format!("apiKey={key}&v=1.16.1&c=test&f=json");
        let body = call(format!("/rest/koanInvite?username=sarita&{sarita}")).await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            v["subsonic-response"]["invite"]["username"], "sarita",
            "{body}"
        );
        assert!(
            v["subsonic-response"]["invite"]["password"].is_null(),
            "{body}"
        );
        for path in [
            "koanInvite?username=owner&",
            "koanInvite?username=sarita&reset=true&",
        ] {
            let body = call(format!("/rest/{path}{sarita}")).await;
            assert!(body.contains("\"code\":50"), "{path}: {body}");
        }
        let body = call(format!("/rest/koanJoin?invite={qr_token}&name=iPad&f=json")).await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            v["subsonic-response"]["join"]["username"], "sarita",
            "{body}"
        );
        let body = call("/rest/koanJoin?invite=forged&f=json".to_owned()).await;
        assert!(body.contains("not valid"), "{body}");

        // A key gives itself up.
        let body = call(format!("/rest/koanJoin?invite={token}&name=spare&f=json")).await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let spare = v["subsonic-response"]["join"]["apiKey"].as_str().unwrap();
        let body = call(format!("/rest/koanRevokeKey?apiKey={spare}&f=json")).await;
        assert!(body.contains("\"ok\""), "{body}");
        let body = call(format!("/rest/ping?apiKey={spare}&v=1.16.1&c=test")).await;
        assert!(body.contains("code=\"44\""), "{body}");
        let body = call(format!("/rest/koanRevokeKey?{owner}")).await;
        assert!(body.contains("sign in with the key"), "{body}");

        // Inviting again never reads the password back.
        let body = call(format!("/rest/koanInvite?username=sarita&{owner}")).await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(
            v["subsonic-response"]["invite"]["token"].is_string(),
            "{body}"
        );
        assert!(
            v["subsonic-response"]["invite"]["password"].is_null(),
            "{body}"
        );

        // A reset gives a new one, signs the invited device out, and withdraws
        // the link already sent.
        let body = call(format!(
            "/rest/koanInvite?username=sarita&reset=true&{owner}"
        ))
        .await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let reset = v["subsonic-response"]["invite"]["password"]
            .as_str()
            .unwrap();
        assert_ne!(reset, password);
        let body = call(format!("/rest/ping?apiKey={key}&v=1.16.1&c=test")).await;
        assert!(body.contains("code=\"44\""), "{body}");
        let body = call(format!("/rest/koanJoin?invite={token}&name=late&f=json")).await;
        assert!(body.contains("not valid"), "{body}");

        let body = call(format!(
            "/rest/koanSetUserRole?username=owner&role=user&{owner}"
        ))
        .await;
        assert!(body.contains("last admin"), "{body}");
        call(format!(
            "/rest/koanSetUserRole?username=sarita&role=user&{owner}"
        ))
        .await;
        let body = call(format!("/rest/koanUsers?{owner}")).await;
        let users: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(
            users["subsonic-response"]["users"]["user"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!({"role": "user", "username": "sarita"})),
            "{body}"
        );

        let body = call(format!("/rest/koanDeleteUser?username=owner&{owner}")).await;
        assert!(body.contains("cannot delete itself"), "{body}");
        call(format!("/rest/koanDeleteUser?username=sarita&{owner}")).await;
        let body = call(format!("/rest/koanUsers?{owner}")).await;
        assert!(!body.contains("sarita"), "{body}");
    }
}
