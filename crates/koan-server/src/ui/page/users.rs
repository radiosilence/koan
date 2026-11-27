//! Users, for admins: make an account, change its access or password, turn
//! off its two-step sign-in, delete it, and produce an invite for it: the
//! link and the email to send it in. The server sends nothing itself; the
//! admin sends the email from their own client. Every route here sits behind
//! the admin layer.

use askama::Template;
use axum::Extension;
use axum::extract::{Path, Query, State};
use axum::response::Response;
use axum::response::sse::Event;
use koan_core::auth::Role;
use koan_core::db::queries::auth::{self as users, UserRow};
use koan_core::invite::{self, AccountError, Invite};
use koan_kit_web as kit;

use super::{Body, Page, Refusal, Section};
use crate::auth::AuthUser;
use crate::html::{Html, Render};
use crate::share::blocking;
use crate::ui::{Logged, Sent, UiState, flash_bad, open, patch, paths};

/// The access levels, as the UI names them.
const ROLES: [(Role, &str); 3] = [
    (Role::Readonly, "Listen only"),
    (Role::User, "Listen and edit"),
    (Role::Admin, "Admin"),
];

/// The list of accounts.
const LIST: &str = "users";
/// Where an invite or a password form opens, under the form making accounts.
const PANEL: &str = "user-panel";
/// The form making accounts.
const NEW: &str = "user-new";

pub(in crate::ui) struct Users;

/// Every account, and which have a second factor.
#[derive(Default)]
struct Listing {
    rows: Vec<UserRow>,
    two_step: std::collections::HashSet<i64>,
}

fn listing(s: &UiState) -> Option<Listing> {
    let db = open(&s.pool)?;
    Some(Listing {
        rows: users::list_users(&db.conn).logged()?,
        two_step: koan_auth::second_factor::enabled_users(&db.conn).logged()?,
    })
}

/// Every account, with its access, an invite, and for others a new password,
/// two-step sign-in turned off, or deletion.
#[derive(Template)]
#[template(path = "users/list.html")]
struct List<'a> {
    listing: &'a Listing,
    me: i64,
}

impl List<'_> {
    fn id(&self) -> &'static str {
        LIST
    }

    fn two_step(&self, u: &UserRow) -> bool {
        self.listing.two_step.contains(&u.id)
    }

    fn role(&self, u: &UserRow) -> kit::Select<'static> {
        kit::Select::new("role")
            .options(ROLES.iter().map(|(r, label)| (r.as_str(), *label)))
            .selected(u.role.as_str())
            .label("Access")
            .compact()
            .data("url", paths::user_role(u.id))
            .datastar(
                "on:change",
                "@post(el.dataset.url, {payload: {role: el.value}})",
            )
    }

    fn invite(&self, u: &UserRow) -> kit::Button<'static> {
        kit::Button::standard("Invite").post(paths::user_invite(u.id, false))
    }

    fn password(&self, u: &UserRow) -> kit::Button<'static> {
        kit::Button::quiet("Password").post(paths::user_password_form(u.id))
    }

    fn two_step_off(&self, u: &UserRow) -> kit::Button<'static> {
        kit::Button::quiet("Two-step off")
            .post(paths::user_two_step_remove(u.id))
            .confirm(
                format!(
                    "Turn off two-step sign-in for {}? Their browsers are signed out, and their password alone signs them in.",
                    u.username
                ),
                "Turn it off",
            )
    }

    fn delete(&self, u: &UserRow) -> kit::Button<'static> {
        kit::Button::quiet("Delete")
            .bad()
            .post(paths::user_delete(u.id))
            .confirm(
                format!(
                    "Delete {}? Their devices stop working and their playlists and favourites go.",
                    u.username
                ),
                "Delete",
            )
    }
}

fn list(listing: &Listing, me: i64) -> Html {
    List { listing, me }.html()
}

/// The form making accounts: its own region, replaced once it is used so the
/// name typed into it goes.
#[derive(Template)]
#[template(path = "users/new.html")]
struct New;

impl New {
    fn id(&self) -> &'static str {
        NEW
    }

    fn form(&self) -> kit::Form<'static> {
        kit::Form::post(paths::users())
    }

    fn username(&self) -> kit::Input<'static> {
        kit::Input::text("username")
            .placeholder("Username")
            .label("Username")
            .attr("maxlength", "64")
            .attr("autocapitalize", "none")
            .attr("spellcheck", "false")
            .autocomplete("off")
            .required()
            .class("flex-1")
    }

    fn role(&self) -> kit::Select<'static> {
        kit::Select::new("role")
            .options(ROLES.iter().map(|(r, label)| (r.as_str(), *label)))
            .selected(Role::Readonly.as_str())
            .label("Access")
    }
}

/// Where an invite or a password form opens: empty, or one of them.
#[derive(Template)]
#[template(path = "users/panel.html")]
struct Panel {
    shown: Html,
}

impl Panel {
    fn id(&self) -> &'static str {
        PANEL
    }
}

fn panel(shown: Html) -> Event {
    patch(&Panel { shown }.html())
}

/// An invite: the link, as text and a QR code, and the email to send it in.
#[derive(Template)]
#[template(path = "users/invite.html")]
struct InvitePanel<'a> {
    invite: &'a Invite,
    link: String,
    qr: Option<kit::Qr<'static>>,
    /// The email as rich text, for the clipboard.
    email: Html,
}

impl InvitePanel<'_> {
    fn copy(&self) -> kit::CopyField<'static> {
        kit::CopyField::new("Invite link", self.link.clone(), "Copy link").copied("Copied")
    }
}

/// The invite email, with the link as a button, for pasting into a mail
/// client as rich text. A template, so every value in it is escaped as a
/// page's is.
#[derive(Template)]
#[template(path = "users/email.html")]
struct Email<'a> {
    invite: &'a Invite,
    link: &'a str,
}

impl Email<'_> {
    fn app_store(&self) -> &'static str {
        invite::APP_STORE
    }

    fn mac(&self) -> &'static str {
        invite::MAC_DOWNLOAD
    }
}

fn invite_panel(invite: &Invite) -> Html {
    let link = invite.link();
    InvitePanel {
        invite,
        qr: kit::Qr::new(invite.qr_link()).map(|q| q.label("Invite link as a QR code")),
        email: Email {
            invite,
            link: &link,
        }
        .html(),
        link,
    }
    .html()
}

/// An invite carrying tokens signed with this server's key: the link's, and
/// the QR code's, which is short-lived.
fn token_invite(
    s: &UiState,
    server: &str,
    id: i64,
    username: &str,
    password: Option<&str>,
) -> Result<Invite, AccountError> {
    let keys = crate::auth::signing_keys().map_err(|e| AccountError::Other(Box::new(e)))?;
    let db = open(&s.pool).ok_or_else(|| AccountError::Other("no database".into()))?;
    let token = invite::mint_token(&db.conn, &keys.0, id, invite::TOKEN_TTL_SECS)?;
    let qr_token = invite::mint_token(&db.conn, &keys.0, id, invite::QR_TOKEN_TTL_SECS)?;
    Ok(Invite::with_token(server, username, &token, password).with_qr_token(Some(&qr_token)))
}

/// The page. `list` is the rendered account list, absent when the server
/// runs without sign-in.
#[derive(Template)]
#[template(path = "users/page.html")]
struct UsersPage {
    list: Option<Html>,
}

impl UsersPage {
    fn new(&self) -> Html {
        New.html()
    }

    fn panel(&self) -> Html {
        Panel {
            shown: Html::default(),
        }
        .html()
    }
}

impl Page for Users {
    const SECTION: Section = Section::Account;
    type Args = ();

    async fn body(s: &UiState, user: &AuthUser, (): ()) -> Result<Body, Refusal> {
        let list = if s.auth_enabled {
            let st = s.clone();
            let rows = blocking(move || listing(&st))
                .await
                .ok_or(Refusal::Unavailable)?;
            Some(self::list(&rows, user.user_id))
        } else {
            None
        };
        Ok(Body::of("Users", &UsersPage { list }))
    }
}

const NO_ADDRESS: &str = "The server's address is unknown: set server.url.";

#[derive(serde::Deserialize)]
pub(in crate::ui) struct NewUser {
    username: String,
    role: String,
}

pub(in crate::ui) async fn create(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    axum::Form(f): axum::Form<NewUser>,
) -> Response {
    if !s.auth_enabled {
        return flash_bad("This server runs without sign-in, so it has no accounts.");
    }
    let Ok(role) = f.role.parse::<Role>() else {
        return flash_bad("Unknown access level.");
    };
    let Some(server) = s.server_url.clone() else {
        return flash_bad(NO_ADDRESS);
    };
    let username = f.username.trim().to_owned();
    let me = user.user_id;
    let made = blocking(move || {
        let db = open(&s.pool)?;
        let made = invite::create_account(&db.conn, &username, role)
            .and_then(|made| token_invite(&s, &server, made.id, &username, Some(&made.password)));
        drop(db);
        Some((made, listing(&s)?))
    })
    .await;
    let Some((made, rows)) = made else {
        return flash_bad("The account could not be made.");
    };
    match made {
        Ok(i) => askama_datastar::events([
            panel(invite_panel(&i)),
            patch(&list(&rows, me)),
            New.html()
                .patch()
                .mode(askama_datastar::ElementPatchMode::Replace)
                .into(),
        ]),
        Err(e) => flash_bad(&e.to_string()),
    }
}

#[derive(serde::Deserialize, Default)]
#[serde(default)]
pub(in crate::ui) struct InviteQuery {
    reset: bool,
}

pub(in crate::ui) async fn invite(
    State(s): State<UiState>,
    Path(id): Path<i64>,
    Query(q): Query<InviteQuery>,
) -> Response {
    let Some(server) = s.server_url.clone() else {
        return flash_bad(NO_ADDRESS);
    };
    let made = blocking(move || {
        let db = open(&s.pool)?;
        let row = users::get_user_by_id(&db.conn, id).logged()??;
        let password = if q.reset {
            match invite::set_password(&db.conn, &row.username, None) {
                Ok(p) => {
                    crate::clients::registry().disconnect(&row.username);
                    Some(p)
                }
                Err(e) => return Some(Err(e)),
            }
        } else {
            None
        };
        drop(db);
        Some(token_invite(
            &s,
            &server,
            row.id,
            &row.username,
            password.as_deref(),
        ))
    })
    .await;
    match made {
        Some(Ok(i)) => askama_datastar::events([panel(invite_panel(&i))]),
        Some(Err(e)) => flash_bad(&e.to_string()),
        None => flash_bad("No such account."),
    }
}

/// The form asking for an account's new password. Its own route and its own
/// form, so a value typed for one account and never sent cannot reach
/// another's.
#[derive(Template)]
#[template(path = "users/password.html")]
struct PasswordForm<'a> {
    id: i64,
    name: &'a str,
}

impl PasswordForm<'_> {
    fn form(&self) -> kit::Form<'static> {
        kit::Form::post(paths::user_password(self.id))
    }

    fn field(&self) -> kit::Input<'static> {
        kit::Input::password("password")
            .placeholder(format!("New password for {}", self.name))
            .label(format!("New password for {}", self.name))
            .attr("minlength", "8")
            .autocomplete("new-password")
            .required()
            .class("flex-1")
    }

    fn generate(&self) -> kit::Button<'static> {
        kit::Button::quiet("Generate one and invite").post(paths::user_invite(self.id, true))
    }
}

pub(in crate::ui) async fn password_form(
    State(s): State<UiState>,
    Path(id): Path<i64>,
) -> Response {
    let row = blocking(move || users::get_user_by_id(&open(&s.pool)?.conn, id).logged()?).await;
    let Some(row) = row else {
        return flash_bad("No such account.");
    };
    let form = PasswordForm {
        id,
        name: &row.username,
    };
    askama_datastar::events([panel(form.html())])
}

#[derive(serde::Deserialize)]
pub(in crate::ui) struct NewPassword {
    password: String,
}

/// Set the password the form posts.
pub(in crate::ui) async fn set_password(
    State(s): State<UiState>,
    Path(id): Path<i64>,
    axum::Form(NewPassword { password }): axum::Form<NewPassword>,
) -> Response {
    let done = blocking(move || {
        let db = open(&s.pool)?;
        let row = users::get_user_by_id(&db.conn, id).logged()??;
        let done = invite::set_password(&db.conn, &row.username, Some(&password));
        if done.is_ok() {
            crate::clients::registry().disconnect(&row.username);
        }
        Some((row, done))
    })
    .await;
    match done {
        Some((row, Ok(_))) => askama_datastar::events([
            panel(Html::default()),
            kit::Flash::ok(format!(
                "{}'s password is changed, and their devices are signed out.",
                row.username
            ))
            .html()
            .patch()
            .into(),
        ]),
        Some((_, Err(e))) => flash_bad(&e.to_string()),
        None => flash_bad("No such account."),
    }
}

#[derive(serde::Deserialize)]
pub(in crate::ui) struct NewRole {
    role: String,
}

pub(in crate::ui) async fn set_role(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
    Sent(NewRole { role }): Sent<NewRole>,
) -> Response {
    let Ok(role) = role.parse::<Role>() else {
        return flash_bad("Unknown access level.");
    };
    let me = user.user_id;
    let done = blocking(move || {
        let db = open(&s.pool)?;
        let row = users::get_user_by_id(&db.conn, id).logged()??;
        let done = invite::set_role(&db.conn, &row.username, role);
        drop(db);
        Some((done, listing(&s)?))
    })
    .await;
    match done {
        Some((Ok(()), rows)) => askama_datastar::events([patch(&list(&rows, me))]),
        // The list drawn again too, so the select goes back to what is stored.
        Some((Err(e), rows)) => askama_datastar::events([
            patch(&list(&rows, me)),
            kit::Flash::bad(e.to_string()).html().patch().into(),
        ]),
        None => flash_bad("No such account."),
    }
}

pub(in crate::ui) async fn delete(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
) -> Response {
    if id == user.user_id {
        return flash_bad("You cannot delete your own account here.");
    }
    let me = user.user_id;
    let done = blocking(move || {
        let db = open(&s.pool)?;
        let row = users::get_user_by_id(&db.conn, id).logged()??;
        let done = invite::delete_account(&db.conn, &row.username);
        if done.is_ok() {
            crate::clients::registry().disconnect(&row.username);
        }
        drop(db);
        Some((done, listing(&s)?))
    })
    .await;
    match done {
        Some((Ok(()), rows)) => askama_datastar::events([
            patch(&list(&rows, me)),
            panel(Html::default()),
            kit::Confirm::close().into(),
        ]),
        Some((Err(e), _)) => flash_bad(&e.to_string()),
        None => flash_bad("No such account."),
    }
}

/// Turn an account's two-step sign-in off, for someone who lost their app and
/// keys: their factors and recovery codes go, and so do their browsers'
/// sessions. Their apps, signed in with keys, stay signed in.
pub(in crate::ui) async fn remove_second_factor(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
) -> Response {
    let me = user.user_id;
    let done = blocking(move || {
        let db = open(&s.pool)?;
        let row = users::get_user_by_id(&db.conn, id).logged()??;
        koan_auth::second_factor::turn_off(&db.conn, row.id).logged()?;
        drop(db);
        Some((row, listing(&s)?))
    })
    .await;
    match done {
        Some((row, rows)) => askama_datastar::events([
            patch(&list(&rows, me)),
            kit::Confirm::close().into(),
            kit::Flash::ok(format!(
                "{} signs in with a password alone again, and their browsers are signed out.",
                row.username
            ))
            .html()
            .patch()
            .into(),
        ]),
        None => flash_bad("No such account."),
    }
}

#[cfg(test)]
mod tests {
    use crate::ui::fixture::*;

    #[tokio::test]
    async fn admins_create_invite_and_remove_accounts() {
        static CONFIG: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
        koan_core::config::set_config_dir(
            CONFIG.get_or_init(|| tempfile::tempdir().unwrap()).path(),
        );
        let f = setup_at(true, Some(ORIGIN));
        let db = Database::open(f.state.pool.path()).unwrap();
        let boss = queries::auth::create_user(&db.conn, "boss", "sesame", Role::Admin).unwrap();
        let token = |id: i64, name: &str, role: Role| {
            auth::mint_access_token(&f.state.private_pem, id, name, role, 900).unwrap()
        };
        let admin = token(boss, "boss", Role::Admin);
        let post = |uri: &str, body: &str, access: &str| {
            Request::post(uri)
                .header(header::HOST, HOST)
                .header(header::COOKIE, format!("koan_access={access}"))
                .header(header::CONTENT_TYPE, "application/json")
                .header("datastar-request", "true")
                .body(Body::from(body.to_owned()))
                .unwrap()
        };
        let page = |access: &str| {
            get("/users")
                .header(header::COOKIE, format!("koan_access={access}"))
                .body(Body::empty())
                .unwrap()
        };

        let r = send(&f.app, page(&access_token(&f.state))).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN);
        let r = send(
            &f.app,
            post("/users", r#"{"newuser":"x"}"#, &access_token(&f.state)),
        )
        .await;
        assert_eq!(r.status, StatusCode::FORBIDDEN);
        // A token minted while alice was an admin: the account's role now governs.
        let r = send(&f.app, page(&token(1, "alice", Role::Admin))).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN);

        let r = send(&f.app, page(&admin)).await;
        assert_eq!(r.status, StatusCode::OK);
        assert!(
            r.body.contains("alice") && r.body.contains("boss"),
            "{}",
            r.body
        );

        let r = send(
            &f.app,
            post(
                "/users",
                r#"{"newuser":"sarita","newrole":"readonly"}"#,
                &admin,
            ),
        )
        .await;
        let link = r
            .body
            .split("id=\"invite-link\" readonly value=\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .unwrap_or_else(|| panic!("no link in {}", r.body))
            .replace("&#38;", "&");
        let invite = koan_core::invite::Invite::parse(&link).unwrap();
        assert_eq!(invite.server, ORIGIN);
        assert_eq!(invite.username, "sarita");
        assert_eq!(invite.password, None);
        let password = r
            .body
            .split("<dt>Password</dt><dd><code>")
            .nth(1)
            .and_then(|rest| rest.split('<').next())
            .unwrap_or_else(|| panic!("no password in {}", r.body))
            .to_owned();
        let row = queries::auth::get_user_by_username(&db.conn, "sarita")
            .unwrap()
            .unwrap();
        assert_eq!(row.role, Role::Readonly);
        auth::verify_password(&password, &row.password_hash).unwrap();

        // The link's token is signed with this server's key and redeems for a key.
        let joined = koan_core::invite::redeem(
            &db.conn,
            &crate::auth::signing_keys().unwrap().1,
            invite.token.as_deref().unwrap(),
            "phone",
        )
        .unwrap();
        assert_eq!(joined.username, "sarita");

        // Inviting again shows no password: the server cannot read one back.
        let r = send(
            &f.app,
            post(&format!("/users/{}/invite", row.id), "{}", &admin),
        )
        .await;
        assert!(r.body.contains("id=\"invite-link\""), "{}", r.body);
        assert!(
            r.body
                .contains("aria-label=\"Invite link as a QR code\"><svg"),
            "{}",
            r.body
        );
        assert!(!r.body.contains("<dt>Password</dt>"), "{}", r.body);

        // A reset does show one, and signs the account's devices out.
        let alice_link = open_link("alice");
        let r = send(&f.app, post("/users/1/invite?reset=true", "{}", &admin)).await;
        assert!(r.body.contains("<dt>Password</dt>"), "{}", r.body);
        assert_closed(alice_link);

        // So does a password the admin chooses. The row's button asks for the
        // form, and a value typed into another account's form and never
        // submitted, which Datastar posts along with it, changes nothing.
        let before = queries::auth::get_user_by_id(&db.conn, 1)
            .unwrap()
            .unwrap()
            .password_hash;
        let r = send(
            &f.app,
            post(
                "/users/1/password/form",
                r#"{"setpassword":"typed for someone else"}"#,
                &admin,
            ),
        )
        .await;
        assert!(r.body.contains("data-bind:setpassword"), "{}", r.body);
        assert!(r.body.contains(r#"{"setpassword":""}"#), "{}", r.body);
        let after = queries::auth::get_user_by_id(&db.conn, 1)
            .unwrap()
            .unwrap()
            .password_hash;
        assert_eq!(before, after);
        let r = send(
            &f.app,
            post("/users/1/password", r#"{"setpassword":"short"}"#, &admin),
        )
        .await;
        assert!(r.body.contains("at least 8"), "{}", r.body);
        let r = send(
            &f.app,
            post(
                "/users/1/password",
                r#"{"setpassword":"correct horse"}"#,
                &admin,
            ),
        )
        .await;
        assert!(r.body.contains("password is changed"), "{}", r.body);
        let alice = queries::auth::get_user_by_id(&db.conn, 1).unwrap().unwrap();
        auth::verify_password("correct horse", &alice.password_hash).unwrap();
        assert!(
            queries::api_keys::authenticate_api_key(&db.conn, &joined.api_key)
                .unwrap()
                .is_some()
        );

        let r = send(
            &f.app,
            post(&format!("/users/{}/role?role=user", row.id), "{}", &admin),
        )
        .await;
        assert_eq!(r.status, StatusCode::OK);
        let r = send(
            &f.app,
            post(&format!("/users/{boss}/role?role=user",), "{}", &admin),
        )
        .await;
        assert!(r.body.contains("last admin"), "{}", r.body);
        assert_eq!(
            queries::auth::get_user_by_username(&db.conn, "sarita")
                .unwrap()
                .unwrap()
                .role,
            Role::User
        );

        let r = send(&f.app, post(&format!("/users/{boss}/delete"), "{}", &admin)).await;
        assert!(r.body.contains("own account"), "{}", r.body);
        let sarita_link = open_link("sarita");
        send(
            &f.app,
            post(&format!("/users/{}/delete", row.id), "{}", &admin),
        )
        .await;
        assert!(
            queries::auth::get_user_by_username(&db.conn, "sarita")
                .unwrap()
                .is_none()
        );
        assert_closed(sarita_link);
    }

    #[tokio::test]
    async fn a_username_reaches_the_delete_confirmation_as_data() {
        let f = setup(true);
        let db = Database::open(f.state.pool.path()).unwrap();
        let boss = queries::auth::create_user(&db.conn, "boss", "sesame", Role::Admin).unwrap();
        queries::auth::create_user(&db.conn, "x');alert(1)//", "sesame", Role::User).unwrap();
        let admin =
            auth::mint_access_token(&f.state.private_pem, boss, "boss", Role::Admin, 900).unwrap();
        let r = send(
            &f.app,
            get("/users")
                .header(header::COOKIE, format!("koan_access={admin}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(r.status, StatusCode::OK);
        let button = r
            .body
            .split("<button")
            .find(|b| b.contains("data-username=\"x&#39;);alert(1)//\""))
            .unwrap_or_else(|| panic!("{}", r.body));
        let expression = button
            .split("data-on:click=\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .unwrap();
        assert!(expression.contains("/delete')"), "{expression}");
        assert!(!expression.contains("alert"), "{expression}");
    }
}
