//! The prologue every endpoint shares: authenticate, open the database, run
//! the work off the async runtime and answer in the format asked for.

use super::*;

/// Authenticate, open the database, and render — the prologue every browsing
/// endpoint shares. Errors come back in the format the request asked for.
pub(super) fn respond_db(
    state: &AppState,
    auth: &SubsonicParams,
    f: impl FnOnce(&Database, XmlBuilder) -> Result<XmlBuilder, SubsonicError>,
) -> Response {
    respond_db_as(state, auth, Role::Readonly, f)
}

/// As `respond_db`, for an endpoint needing at least `need`.
pub(super) fn respond_db_as(
    state: &AppState,
    auth: &SubsonicParams,
    need: Role,
    f: impl FnOnce(&Database, XmlBuilder) -> Result<XmlBuilder, SubsonicError>,
) -> Response {
    respond_db_user(state, auth, need, |db, _, b| f(db, b))
}

/// As `respond_db_as`, for an endpoint that reads or writes the caller's own
/// favourites, playlists or history. It is handed the caller's user id.
pub(super) fn respond_db_user(
    state: &AppState,
    auth: &SubsonicParams,
    need: Role,
    f: impl FnOnce(&Database, i64, XmlBuilder) -> Result<XmlBuilder, SubsonicError>,
) -> Response {
    respond_db_caller(state, auth, need, |db, caller, b| f(db, caller.user_id, b))
}

/// As `respond_db_user`, handing over the whole caller.
pub(super) fn respond_db_caller(
    state: &AppState,
    auth: &SubsonicParams,
    need: Role,
    f: impl FnOnce(&Database, &Caller, XmlBuilder) -> Result<XmlBuilder, SubsonicError>,
) -> Response {
    let json = auth.wants_json();
    let result = validate_auth(auth, state)
        .and_then(|caller| {
            if need == Role::Readonly {
                Ok(caller)
            } else {
                require_write(caller.role).map(|()| caller)
            }
        })
        .and_then(|caller| Ok((state.open_db()?, caller)))
        .and_then(|(db, caller)| f(&db, &caller, SubsonicResponse::ok(json)));
    match result {
        Ok(builder) => builder.build(),
        Err(e) => SubsonicResponse::error(json, &e),
    }
}

/// As `respond_db`, for endpoints that never touch the database. They are
/// handed who is asking.
pub(super) fn respond(
    state: &AppState,
    auth: &SubsonicParams,
    f: impl FnOnce(&Caller, XmlBuilder) -> Result<XmlBuilder, SubsonicError>,
) -> Response {
    let json = auth.wants_json();
    match validate_auth(auth, state).and_then(|caller| f(&caller, SubsonicResponse::ok(json))) {
        Ok(builder) => builder.build(),
        Err(e) => SubsonicResponse::error(json, &e),
    }
}

/// Run a handler's synchronous work — authentication, SQLite, argon2, cover
/// decoding — on the blocking pool. Done on a runtime worker it holds that
/// worker until it finishes, and a few concurrent clients then stall every
/// other route in the process, the web UI included.
pub(super) async fn offload_response(f: impl FnOnce() -> Response + Send + 'static) -> Response {
    tokio::task::spawn_blocking(f)
        .await
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// As [`offload_response`], for work whose result a handler goes on to use.
pub(super) async fn offload<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, SubsonicError> + Send + 'static,
) -> Result<T, SubsonicError> {
    tokio::task::spawn_blocking(f)
        .await
        .unwrap_or_else(|e| Err(SubsonicError::internal(e.to_string())))
}

/// Prologue for the two endpoints that answer with bytes rather than a
/// document, and so cannot go through `respond_db`.
pub(super) fn authed_db<'a>(
    state: &'a AppState,
    auth: &SubsonicParams,
) -> Result<Handle<'a>, SubsonicError> {
    validate_auth(auth, state)?;
    state.open_db()
}
