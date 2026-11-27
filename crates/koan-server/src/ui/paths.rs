//! Every address the UI answers, in one place: the route patterns the router
//! declares (the constants), and the functions that write an address for a
//! page or a template (relative, since pages link from their `<base href>`,
//! the base path a proxy may put the UI under). Nothing else spells an
//! address: a template or script that needs one reads it from here, through
//! the page that writes it into a `data-` attribute.

// Pages
pub(crate) const HOME: &str = "/";
pub(crate) const ALBUMS: &str = "/albums";
pub(crate) const ALBUM: &str = "/album/{id}";
pub(crate) const ARTISTS: &str = "/artists";
pub(crate) const ARTIST: &str = "/artist/{id}";
pub(crate) const TRACKS: &str = "/tracks";
pub(crate) const PROPERTIES: &str = "/track/{id}/properties";
pub(crate) const PLAYLISTS: &str = "/playlists";
pub(crate) const PLAYLIST: &str = "/playlist/{id}";
pub(crate) const SEARCH: &str = "/search";
pub(crate) const QUEUE: &str = "/queue";
pub(crate) const LIBRARY: &str = "/library";
pub(crate) const RECENTLY_PLAYED: &str = "/recent";
pub(crate) const FAVOURITES: &str = "/favourites";
pub(crate) const HISTORY: &str = "/history";
pub(crate) const ACCOUNT: &str = "/account";
pub(crate) const USERS: &str = "/users";
pub(crate) const ASSISTANTS: &str = "/assistants";
pub(crate) const SCROBBLING: &str = "/scrobbling";
/// Where Assistants was, and where an older `/mcp` sent a browser.
pub(crate) const OLD_ASSISTANTS: &str = "/connect";
/// Where the API keys were, before the Account page held them.
pub(crate) const OLD_KEYS: &str = "/keys";

// The live stream
pub(crate) const LIVE: &str = "/ui/live";

// Actions
pub(crate) const ALBUM_SHARE: &str = "/album/{id}/share";
pub(crate) const ARTIST_SHARE: &str = "/artist/{id}/share";
pub(crate) const FAVOURITE: &str = "/favourite/{kind}/{id}";
pub(crate) const QUEUE_ROWS: &str = "/queue/rows";
pub(crate) const HISTORY_FORGET: &str = "/history/forget";
pub(crate) const RESCAN: &str = "/library/scan";
pub(crate) const CACHES_CLEAR: &str = "/library/caches/clear";
pub(crate) const CACHES_WARM: &str = "/library/caches/warm";
pub(crate) const KEYS: &str = "/account/keys";
pub(crate) const KEY_REVOKE: &str = "/account/keys/{id}/revoke";
pub(crate) const APP_PASSWORDS: &str = "/account/app-passwords";
pub(crate) const APP_PASSWORD_REVOKE: &str = "/account/app-passwords/{id}/revoke";
pub(crate) const TOTP: &str = "/account/two-step/totp";
pub(crate) const TOTP_CONFIRM: &str = "/account/two-step/totp/confirm";
pub(crate) const TOTP_REMOVE: &str = "/account/two-step/totp/remove";
pub(crate) const SECURITY_KEYS: &str = "/account/two-step/keys";
pub(crate) const SECURITY_KEYS_BEGIN: &str = "/account/two-step/keys/begin";
pub(crate) const SECURITY_KEY_REMOVE: &str = "/account/two-step/keys/{id}/remove";
pub(crate) const RECOVERY_CODES: &str = "/account/two-step/recovery-codes";
pub(crate) const LASTFM_CONNECT: &str = "/scrobbling/lastfm/connect";
pub(crate) const LASTFM_RETURN: &str = "/scrobbling/lastfm/return/{state}";
pub(crate) const LASTFM_DISCONNECT: &str = "/scrobbling/lastfm/disconnect";
pub(crate) const LISTENBRAINZ: &str = "/scrobbling/listenbrainz";
pub(crate) const LISTENBRAINZ_DISCONNECT: &str = "/scrobbling/listenbrainz/disconnect";
pub(crate) const USER_INVITE: &str = "/users/{id}/invite";
pub(crate) const USER_PASSWORD: &str = "/users/{id}/password";
pub(crate) const USER_PASSWORD_FORM: &str = "/users/{id}/password/form";
pub(crate) const USER_ROLE: &str = "/users/{id}/role";
pub(crate) const USER_DELETE: &str = "/users/{id}/delete";
pub(crate) const USER_TWO_STEP_REMOVE: &str = "/users/{id}/two-step/remove";

// The player's audio and covers, and the UI's assets
pub(crate) const AUDIO: &str = "/ui/stream/{id}";
pub(crate) const COVER: &str = "/ui/cover/{id}";
pub(crate) const ASSETS: &str = "/ui/assets/{name}";
pub(crate) const FAVICON: &str = "/favicon.ico";
pub(crate) const TOUCH_ICON: &str = "/apple-touch-icon.png";

// Signing in
pub(crate) const LOGIN: &str = "/login";
pub(crate) const LOGIN_SECOND: &str = "/login/second";
pub(crate) const LOGIN_PASSKEY: &str = "/login/passkey";
pub(crate) const LOGIN_PASSKEY_CHALLENGE: &str = "/login/passkey/challenge";
pub(crate) const SETUP: &str = "/setup";
pub(crate) const RESUME: &str = "/auth/resume";
/// Where a page load goes for a session when an authenticating proxy is
/// trusted. A UI path, so the proxy covers it: the paths operators exempt
/// from the proxy (`/auth/login`, `/oauth/token`…) never read the header, and
/// a client's own header reaching them through an exemption signs in no one.
pub(crate) const PROXY_RESUME: &str = "/ui/resume";
pub(crate) const PROXY_RENEW: &str = "/ui/renew";
pub(crate) const RENEW: &str = "/auth/renew";
pub(crate) const SIGN_OUT: &str = "/auth/signout";
pub(crate) const PAIR: &str = "/pair";
pub(crate) const PAIRING: &str = "/pair/{pair}";
pub(crate) const PAIR_APPROVE: &str = "/pair/{pair}/approve";
pub(crate) const PAIR_DECLINE: &str = "/pair/{pair}/decline";

// OAuth
pub(crate) const AUTHORIZE: &str = "/oauth/authorize";
pub(crate) const REGISTER: &str = "/oauth/register";
pub(crate) const TOKEN: &str = "/oauth/token";
pub(crate) const PROTECTED_RESOURCE: &str = "/.well-known/oauth-protected-resource";
pub(crate) const AUTHORIZATION_SERVER: &str = "/.well-known/oauth-authorization-server";

/// A route's address as a page links it: relative, from the `<base href>`.
fn rel(route: &str) -> &str {
    match route.trim_start_matches('/') {
        "" => "./",
        r => r,
    }
}

/// `route` with its one `{…}` parameter filled.
fn fill(route: &str, value: impl std::fmt::Display) -> String {
    let (head, rest) = route.split_once('{').expect("a route with a parameter");
    let (_, tail) = rest.split_once('}').expect("a closed parameter");
    format!("{}{value}{tail}", rel(head))
}

/// `route` with its two parameters filled, in order.
fn fill2(route: &str, a: impl std::fmt::Display, b: impl std::fmt::Display) -> String {
    let (head, rest) = route.split_once('{').expect("a first parameter");
    let (_, rest) = rest.split_once('}').expect("a closed parameter");
    format!("{}{a}{}", rel(head), fill(rest, b).trim_start_matches("./"))
}

/// `route`, relative, with `query` after it where it has anything.
fn with_query(route: &str, query: &str) -> String {
    if query.is_empty() {
        rel(route).to_owned()
    } else {
        format!("{}?{query}", rel(route))
    }
}

fn query<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> String {
    let mut q = form_urlencoded::Serializer::new(String::new());
    for (k, v) in pairs {
        q.append_pair(k, v);
    }
    q.finish()
}

pub(crate) fn home() -> &'static str {
    rel(HOME)
}
pub(crate) fn albums() -> &'static str {
    rel(ALBUMS)
}
/// A browser with `query`, a `Browse` written out.
pub(crate) fn browser(route: &str, query: &str) -> String {
    with_query(route, query)
}
pub(crate) fn album(id: i64) -> String {
    fill(ALBUM, id)
}
pub(crate) fn artists() -> &'static str {
    rel(ARTISTS)
}
pub(crate) fn artist(id: i64) -> String {
    fill(ARTIST, id)
}
pub(crate) fn tracks() -> &'static str {
    rel(TRACKS)
}
pub(crate) fn properties(track: i64) -> String {
    fill(PROPERTIES, track)
}
pub(crate) fn playlists() -> &'static str {
    rel(PLAYLISTS)
}
pub(crate) fn playlist(id: i64) -> String {
    fill(PLAYLIST, id)
}
pub(crate) fn search() -> &'static str {
    rel(SEARCH)
}
pub(crate) fn queue() -> &'static str {
    rel(QUEUE)
}
pub(crate) fn library() -> &'static str {
    rel(LIBRARY)
}
pub(crate) fn recently_played() -> &'static str {
    rel(RECENTLY_PLAYED)
}
pub(crate) fn favourites() -> &'static str {
    rel(FAVOURITES)
}
pub(crate) fn history() -> &'static str {
    rel(HISTORY)
}
pub(crate) fn history_page(page: u32) -> String {
    if page == 0 {
        rel(HISTORY).to_owned()
    } else {
        with_query(HISTORY, &query([("page", page.to_string().as_str())]))
    }
}
pub(crate) fn account() -> &'static str {
    rel(ACCOUNT)
}
pub(crate) fn users() -> &'static str {
    rel(USERS)
}
pub(crate) fn assistants() -> &'static str {
    rel(ASSISTANTS)
}
pub(crate) fn scrobbling() -> &'static str {
    rel(SCROBBLING)
}

pub(crate) fn album_share(album: i64, track: Option<i64>) -> String {
    match track {
        None => fill(ALBUM_SHARE, album),
        Some(t) => format!(
            "{}?{}",
            fill(ALBUM_SHARE, album),
            query([("track", t.to_string().as_str())])
        ),
    }
}
pub(crate) fn artist_share(artist: i64) -> String {
    fill(ARTIST_SHARE, artist)
}
/// Favouriting `kind`'s `id`, or with `on` false unfavouriting it.
pub(crate) fn favourite(kind: &str, id: i64, on: bool) -> String {
    format!(
        "{}?on={}",
        fill2(FAVOURITE, kind, id),
        if on { 1 } else { 0 }
    )
}
pub(crate) fn queue_rows() -> &'static str {
    rel(QUEUE_ROWS)
}
pub(crate) fn history_forget(page: u32) -> String {
    with_query(
        HISTORY_FORGET,
        &query([("page", page.to_string().as_str())]),
    )
}
pub(crate) fn rescan() -> &'static str {
    rel(RESCAN)
}
pub(crate) fn caches_clear() -> &'static str {
    rel(CACHES_CLEAR)
}
pub(crate) fn caches_warm() -> &'static str {
    rel(CACHES_WARM)
}
pub(crate) fn keys() -> &'static str {
    rel(KEYS)
}
pub(crate) fn key_revoke(id: i64) -> String {
    fill(KEY_REVOKE, id)
}
pub(crate) fn app_passwords() -> &'static str {
    rel(APP_PASSWORDS)
}
pub(crate) fn app_password_revoke(id: i64) -> String {
    fill(APP_PASSWORD_REVOKE, id)
}
pub(crate) fn totp() -> &'static str {
    rel(TOTP)
}
pub(crate) fn totp_confirm() -> &'static str {
    rel(TOTP_CONFIRM)
}
pub(crate) fn totp_remove() -> &'static str {
    rel(TOTP_REMOVE)
}
pub(crate) fn security_keys() -> &'static str {
    rel(SECURITY_KEYS)
}
pub(crate) fn security_keys_begin() -> &'static str {
    rel(SECURITY_KEYS_BEGIN)
}
pub(crate) fn security_key_remove(id: i64) -> String {
    fill(SECURITY_KEY_REMOVE, id)
}
pub(crate) fn recovery_codes() -> &'static str {
    rel(RECOVERY_CODES)
}
pub(crate) fn lastfm_connect() -> &'static str {
    rel(LASTFM_CONNECT)
}
pub(crate) fn lastfm_return(state: &str) -> String {
    fill(LASTFM_RETURN, encode(state))
}
pub(crate) fn lastfm_disconnect() -> &'static str {
    rel(LASTFM_DISCONNECT)
}
pub(crate) fn listenbrainz() -> &'static str {
    rel(LISTENBRAINZ)
}
pub(crate) fn listenbrainz_disconnect() -> &'static str {
    rel(LISTENBRAINZ_DISCONNECT)
}
pub(crate) fn user_invite(id: i64, reset: bool) -> String {
    if reset {
        format!("{}?reset=true", fill(USER_INVITE, id))
    } else {
        fill(USER_INVITE, id)
    }
}
pub(crate) fn user_password(id: i64) -> String {
    fill(USER_PASSWORD, id)
}
pub(crate) fn user_password_form(id: i64) -> String {
    fill(USER_PASSWORD_FORM, id)
}
pub(crate) fn user_role(id: i64) -> String {
    fill(USER_ROLE, id)
}
pub(crate) fn user_delete(id: i64) -> String {
    fill(USER_DELETE, id)
}
pub(crate) fn user_two_step_remove(id: i64) -> String {
    fill(USER_TWO_STEP_REMOVE, id)
}
pub(crate) fn audio(track: i64) -> String {
    fill(AUDIO, track)
}
/// A record's cover at `size`, at `version`: when its files last changed, so
/// the address changes whenever the art might have.
pub(crate) fn cover(album: i64, size: u32, version: i64) -> String {
    format!("{}?size={size}&v={version}", fill(COVER, album))
}
/// One of the UI's assets, as `/ui/assets/<name>` serves it: relative, as a
/// page asks for it.
pub(crate) fn asset(name: &str) -> String {
    fill(ASSETS, name)
}
pub(crate) fn sign_out() -> &'static str {
    rel(SIGN_OUT)
}
pub(crate) fn login() -> &'static str {
    rel(LOGIN)
}
pub(crate) fn login_second() -> &'static str {
    rel(LOGIN_SECOND)
}
pub(crate) fn login_passkey() -> &'static str {
    rel(LOGIN_PASSKEY)
}
pub(crate) fn login_passkey_challenge() -> &'static str {
    rel(LOGIN_PASSKEY_CHALLENGE)
}
pub(crate) fn setup() -> &'static str {
    rel(SETUP)
}
pub(crate) fn renew() -> &'static str {
    rel(RENEW)
}
pub(crate) fn proxy_renew() -> &'static str {
    rel(PROXY_RENEW)
}
pub(crate) fn pair() -> &'static str {
    rel(PAIR)
}
pub(crate) fn pairing(pair: &str) -> String {
    fill(PAIRING, encode(pair))
}
pub(crate) fn pair_approve(pair: &str) -> String {
    fill(PAIR_APPROVE, encode(pair))
}
pub(crate) fn pair_decline(pair: &str) -> String {
    fill(PAIR_DECLINE, encode(pair))
}
pub(crate) fn authorize() -> &'static str {
    rel(AUTHORIZE)
}

/// One path segment, percent-encoded.
fn encode(segment: &str) -> String {
    form_urlencoded::byte_serialize(segment.as_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    /// Every address the functions write reaches the route it is for: a
    /// misspelt address fails here rather than as a dead link.
    #[tokio::test]
    async fn every_address_matches_its_route() {
        use super::*;
        let written: Vec<(&str, String)> = vec![
            (HOME, home().into()),
            (ALBUMS, albums().into()),
            (ALBUMS, browser(ALBUMS, "sort=year&q=a+b")),
            (ALBUM, album(7)),
            (ARTISTS, artists().into()),
            (ARTIST, artist(7)),
            (TRACKS, tracks().into()),
            (PROPERTIES, properties(7)),
            (PLAYLISTS, playlists().into()),
            (PLAYLIST, playlist(7)),
            (SEARCH, search().into()),
            (QUEUE, queue().into()),
            (LIBRARY, library().into()),
            (RECENTLY_PLAYED, recently_played().into()),
            (FAVOURITES, favourites().into()),
            (HISTORY, history().into()),
            (HISTORY, history_page(2)),
            (ACCOUNT, account().into()),
            (USERS, users().into()),
            (ASSISTANTS, assistants().into()),
            (SCROBBLING, scrobbling().into()),
            (ALBUM_SHARE, album_share(7, None)),
            (ALBUM_SHARE, album_share(7, Some(3))),
            (ARTIST_SHARE, artist_share(7)),
            (FAVOURITE, favourite("album", 7, true)),
            (QUEUE_ROWS, queue_rows().into()),
            (HISTORY_FORGET, history_forget(1)),
            (RESCAN, rescan().into()),
            (CACHES_CLEAR, caches_clear().into()),
            (CACHES_WARM, caches_warm().into()),
            (KEYS, keys().into()),
            (KEY_REVOKE, key_revoke(7)),
            (APP_PASSWORDS, app_passwords().into()),
            (APP_PASSWORD_REVOKE, app_password_revoke(7)),
            (TOTP, totp().into()),
            (TOTP_CONFIRM, totp_confirm().into()),
            (TOTP_REMOVE, totp_remove().into()),
            (SECURITY_KEYS, security_keys().into()),
            (SECURITY_KEYS_BEGIN, security_keys_begin().into()),
            (SECURITY_KEY_REMOVE, security_key_remove(7)),
            (RECOVERY_CODES, recovery_codes().into()),
            (LASTFM_CONNECT, lastfm_connect().into()),
            (LASTFM_RETURN, lastfm_return("a/b c")),
            (LASTFM_DISCONNECT, lastfm_disconnect().into()),
            (LISTENBRAINZ, listenbrainz().into()),
            (LISTENBRAINZ_DISCONNECT, listenbrainz_disconnect().into()),
            (USER_INVITE, user_invite(7, true)),
            (USER_PASSWORD, user_password(7)),
            (USER_PASSWORD_FORM, user_password_form(7)),
            (USER_ROLE, user_role(7)),
            (USER_DELETE, user_delete(7)),
            (USER_TWO_STEP_REMOVE, user_two_step_remove(7)),
            (AUDIO, audio(7)),
            (COVER, cover(7, 300, 12)),
            (ASSETS, asset("ui.css")),
            (SIGN_OUT, sign_out().into()),
            (LOGIN, login().into()),
            (LOGIN_SECOND, login_second().into()),
            (LOGIN_PASSKEY, login_passkey().into()),
            (LOGIN_PASSKEY_CHALLENGE, login_passkey_challenge().into()),
            (SETUP, setup().into()),
            (RENEW, renew().into()),
            (PROXY_RENEW, proxy_renew().into()),
            (PAIR, pair().into()),
            (PAIRING, pairing("AB-12")),
            (PAIR_APPROVE, pair_approve("AB-12")),
            (PAIR_DECLINE, pair_decline("AB-12")),
            (AUTHORIZE, authorize().into()),
        ];
        let mut router = axum::Router::new();
        let mut routes: Vec<&str> = written.iter().map(|(r, _)| *r).collect();
        routes.sort_unstable();
        routes.dedup();
        for route in routes {
            router = router.route(route, axum::routing::any(move || async move { route }));
        }
        for (route, address) in written {
            assert!(!address.starts_with('/'), "{address} is relative");
            let uri = format!("/{}", address.trim_start_matches("./"));
            let res = router
                .clone()
                .oneshot(Request::get(&uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            let body = axum::body::to_bytes(res.into_body(), 1024).await.unwrap();
            assert_eq!(body, route.as_bytes(), "{uri} reaches {route}");
        }
    }
}
