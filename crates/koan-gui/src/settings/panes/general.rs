//! General: what kōan does as an app, before any library or device. Keeping
//! running with the window closed and opening at login, and keeping this
//! copy up to date.

use crate::components::{Button, ButtonKind, Toggle};
use crate::theme::{Icon, Look, Tone, TypeRole};
use koan_kit::app::*;

use crate::settings::Ctx;
use crate::settings::form::{note, page, section, text};

pub struct GeneralPane {
    ctx: Ctx,
}

impl GeneralPane {
    pub fn new(ctx: Ctx) -> Self {
        Self { ctx }
    }
}

impl Render for GeneralPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let Some(settings) = self.ctx.model.read(cx).settings().cloned() else {
            return page("general", &look, Vec::new());
        };
        let model = self.ctx.model.clone();
        let mut sections = vec![background(&settings, &model, &look, cx)];
        sections.extend(updates(&settings, &model, &look, cx));
        page("general", &look, sections)
    }
}

/// Staying reachable with the window closed, as the Mac's "In the
/// background": kept running, and opening at login, which the system owns.
fn background(
    settings: &koan_runtime::Settings,
    model: &Entity<crate::settings::model::SettingsModel>,
    look: &Look,
    cx: &App,
) -> AnyElement {
    use crate::platform::residency::{self, Login};
    let mut rows = Vec::new();
    {
        let model = model.clone();
        let label = if cfg!(target_os = "macos") {
            "Keep running in the menu bar"
        } else {
            "Keep running with the window closed"
        };
        rows.push(
            Toggle::new("keep-running", label, settings.devices_keep_running)
                .on_change(move |on, _, cx| {
                    let on = *on;
                    // Applied once it is in the file: a write that fails
                    // leaves kōan as the file says.
                    model.update(cx, |m, cx| {
                        m.edit(cx, move |s| s.devices_keep_running = on);
                        m.when_written(cx, move |_, cx| residency::apply(on, cx));
                    });
                })
                .into_any_element(),
        );
    }
    let login = residency::login(cx);
    if login != Login::Unavailable {
        rows.push(
            Toggle::new(
                "open-at-login",
                "Open at login",
                matches!(login, Login::On | Login::NeedsApproval),
            )
            .on_change(move |on, _, cx| residency::set_login(*on, cx))
            .into_any_element(),
        );
    }
    if let Some(error) = residency::login_error(cx) {
        rows.push(note(error, Icon::Warning, Tone::Bad, look));
    }
    if login == Login::NeedsApproval {
        rows.push(note(
            "Allow kōan in System Settings ▸ General ▸ Login Items.",
            Icon::Warning,
            Tone::Bad,
            look,
        ));
    }
    let footer = if cfg!(target_os = "macos") {
        "With its window closed, kōan stays in the menu bar, signed in to your server and listening on this network, so your other devices can see and control this Mac. A Mac cannot be woken from another device: once kōan is quit, it is out of reach until kōan is opened again."
    } else {
        "With its window closed, kōan keeps running, signed in to your server and listening on this network, so your other devices can see and control this computer; opening kōan again brings its window back. A computer cannot be woken from another device: once kōan is quit, it is out of reach until kōan is opened again."
    };
    section(Some("In the background"), rows, Some(footer.into()), look)
}

/// Keeping this copy of kōan up to date, where it can be: nothing for a
/// build from source.
fn updates(
    settings: &koan_runtime::Settings,
    model: &Entity<crate::settings::model::SettingsModel>,
    look: &Look,
    cx: &App,
) -> Option<AnyElement> {
    use crate::platform::update::{self, Install, State};
    let install = update::install_kind(cx);
    let told = match &install {
        Install::Unknown | Install::Source => return None,
        Install::Told(why) => Some(*why),
        Install::Bundle { .. } | Install::AppImage(_) => None,
    };
    let label = if told.is_some() {
        "Check for new versions"
    } else {
        "Update automatically"
    };
    let toggle = {
        let model = model.clone();
        Toggle::new("updates-auto", label, settings.updates_auto)
            .on_change(move |on, _, cx| {
                let on = *on;
                model.update(cx, |m, cx| {
                    m.edit(cx, move |s| s.updates_auto = on);
                    m.when_written(cx, move |_, cx| update::follow(on, cx));
                });
            })
            .into_any_element()
    };
    let state = update::state(cx);
    let busy = matches!(state, State::Checking | State::Fetching(_));
    let status = match &state {
        State::Idle => None,
        State::Checking => Some(("Checking…".to_owned(), Tone::Muted)),
        State::Current => Some(("This is the latest version".to_owned(), Tone::Muted)),
        State::Fetching(v) => Some((format!("Downloading kōan {v}…"), Tone::Muted)),
        State::Ready(v) => Some((
            format!("kōan {v} is downloaded, and installed when kōan quits"),
            Tone::Ink,
        )),
        State::Available { version, .. } => Some((format!("kōan {version} is out"), Tone::Ink)),
        State::Failed(e) => Some((e.clone(), Tone::Bad)),
    };
    // One button, following the state: look, take what was found, or read
    // about what this install cannot take itself.
    let button = match state {
        State::Ready(_) => Button::new("updates-restart", ButtonKind::Prominent)
            .label("Update now")
            .on_click(|_, _, cx| update::restart(cx)),
        State::Available { page, .. } => Button::new("updates-page", ButtonKind::Bordered)
            .label("Release notes")
            .on_click(move |_, _, cx| cx.open_url(&page)),
        _ => Button::new("updates-check", ButtonKind::Bordered)
            .label("Check now")
            .disabled(busy)
            .on_click(|_, _, cx| update::check(cx)),
    };
    let mut rows = vec![
        toggle,
        text(
            format!(
                "kōan {} ({})",
                crate::platform::about::VERSION,
                crate::platform::about::BUILD
            ),
            TypeRole::Body,
            Tone::Ink,
            look,
        )
        .into_any_element(),
    ];
    if let Some((status, tone)) = status {
        rows.push(text(status, TypeRole::Fine, tone, look).into_any_element());
    }
    rows.push(crate::settings::form::buttons(
        vec![button.into_any_element()],
        look,
    ));
    let footer = told.map_or_else(
        || {
            "New versions are downloaded from GitHub, checked, and installed when kōan quits."
                .to_owned()
        },
        str::to_owned,
    );
    Some(section(Some("Updates"), rows, Some(footer.into()), look))
}
