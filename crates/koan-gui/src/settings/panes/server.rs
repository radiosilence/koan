//! Server: what this device does about servers. It signs in to one, or, on
//! the desktop, is one, sharing its own library; and what it keeps of a
//! server's library.

use std::time::Duration;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use koan_ffi::{ConnectionInfo, Settings};
use crate::components::{
    Button, ButtonKind, FormRow, Picker, Segmented, Stepper, TextField, Toggle,
};
use crate::theme::{Icon, Look, Tone, TypeRole};

use super::cache::{self, Keeping};
use super::link::LinkDevice;
use crate::mirror::{Mirror, Slice};
use crate::settings::Ctx;
use crate::settings::form::{buttons, copyable, note, page, section, text};
use crate::settings::model::{Uses, bytes, count, describe};
use crate::settings::sheet::{Action, Sheet};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Connect,
    Share,
}

const SYNC_EVERY: [(&str, u64); 5] = [
    ("Startup only", 0),
    ("15 minutes", 15),
    ("Hour", 60),
    ("6 hours", 360),
    ("Day", 1440),
];

/// How long "Copied" stands in for what was copied.
const COPIED: Duration = Duration::from_millis(1500);

pub struct ServerPane {
    ctx: Ctx,
    /// Which side is showing. Starts on what this device is doing; only
    /// choosing changes it.
    mode: Mode,
    url: Entity<InputState>,
    username: Entity<InputState>,
    secret: Entity<InputState>,
    with_key: bool,
    cache_limit: Entity<InputState>,
    /// "Up to a limit" chosen and no limit typed yet: nothing is written,
    /// and nothing deleted, until there is a number.
    choosing_limit: bool,
    cache: Entity<Slice<u64>>,
    copied: Option<&'static str>,
    copy_reset: Option<Task<()>>,
    share: super::share::ShareForm,
    _subscriptions: Vec<Subscription>,
}

impl ServerPane {
    pub fn new(ctx: Ctx, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = ctx.model.read(cx).settings().cloned();
        let field = |placeholder: &'static str, secret: bool, value: String, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .masked(secret)
                    .default_value(value)
            })
        };
        let (url_value, user_value, limit_value) = settings
            .as_ref()
            .map(|s| {
                (
                    s.remote_url.clone(),
                    s.remote_username.clone(),
                    cache::limit_gb(&s.cache_limit),
                )
            })
            .unwrap_or_default();
        let url = field("Server URL", false, url_value, window, cx);
        let username = field("Username", false, user_value, window, cx);
        let secret = field("Password", true, String::new(), window, cx);
        let cache_limit = field("50", false, limit_value, window, cx);
        let cache = cx.global::<Mirror>().cache.clone();
        let share = super::share::ShareForm::new(&ctx, window, cx);
        let subscriptions = vec![
            cx.observe(&ctx.model, |_, _, cx| cx.notify()),
            cx.observe(&ctx.sharing, |_, _, cx| cx.notify()),
            cx.observe(&ctx.connection, |_, _, cx| cx.notify()),
            cx.observe(&cache, |_, _, cx| cx.notify()),
            // An invite, or an address with the account in it, pasted where
            // the address goes.
            cx.subscribe_in(&url, window, |pane, state, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    let typed = state.read(cx).value().to_string();
                    pane.join(&typed, true, window, cx);
                }
            }),
            cx.subscribe_in(&secret, window, |pane, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    pane.sign_in(window, cx);
                }
            }),
            cx.subscribe_in(&cache_limit, window, |pane, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    pane.commit_cache_limit(window, cx);
                }
            }),
        ];
        let mode = if settings.is_some_and(|s| s.share_enabled) {
            Mode::Share
        } else {
            Mode::Connect
        };
        Self {
            ctx,
            mode,
            url,
            username,
            secret,
            with_key: false,
            cache_limit,
            choosing_limit: false,
            cache,
            copied: None,
            copy_reset: None,
            share,
            _subscriptions: subscriptions,
        }
    }

    fn copy(&mut self, which: &'static str, value: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(value));
        self.copied = Some(which);
        cx.notify();
        self.copy_reset = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COPIED).await;
            this.update(cx, |pane, cx| {
                pane.copied = None;
                cx.notify();
            })
            .ok();
        }));
    }

    fn sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let url = self.url.read(cx).value().trim().to_owned();
        let username = self.username.read(cx).value().trim().to_owned();
        let secret = self.secret.read(cx).value().to_string();
        if url.is_empty() || username.is_empty() || secret.is_empty() {
            return;
        }
        // Not kept past the attempt, whatever it comes to.
        self.secret.update(cx, |s, cx| s.set_value("", window, cx));
        let with_key = self.with_key;
        self.ctx
            .model
            .update(cx, |m, cx| m.sign_in(url, username, secret, with_key, cx));
    }

    /// Signs in through an invite. One for another server or account than
    /// the one signed in to asks first, since signing in replaces it.
    fn join(&mut self, text: &str, quiet: bool, window: &mut Window, cx: &mut Context<Self>) {
        let model = self.ctx.model.clone();
        let Some(invite) = model.read(cx).engine().parse_invite(text.to_owned()) else {
            if !quiet {
                model.update(cx, |m, cx| m.report("That isn't a kōan invite.", cx));
            }
            return;
        };
        self.url.update(cx, |s, cx| s.set_value("", window, cx));
        let host = host(&invite.server);
        if self.ctx.sharing.read(cx).serving.is_some() {
            model.update(cx, |m, cx| {
                m.report(
                    format!("This device is sharing its library. Turn sharing off under Shares its library to sign in to {host}."),
                    cx,
                )
            });
            return;
        }
        let Some(settings) = model.read(cx).settings() else { return };
        let elsewhere = settings.remote_url != invite.server
            || settings.remote_username != invite.username;
        if settings.remote_signed_in && elsewhere {
            let current = host_of(settings);
            let join = {
                let model = model.clone();
                let invite = invite.clone();
                move |_: &[String], _: &mut Window, cx: &mut App| {
                    let invite = invite.clone();
                    model.update(cx, |m, cx| m.join_invite(invite, cx))
                }
            };
            let sheet = Sheet::new(format!("Sign in to {host} as {}?", invite.username))
                .message(format!(
                    "kōan is signed in to {current}. Signing in here signs out of it: its tracks, favourites, history, playlists and downloads are removed from this device. Local files stay."
                ))
                .action(Action::new("Sign In", join));
            self.ctx.sheets.update(cx, |s, cx| s.show(sheet, window, cx));
        } else {
            model.update(cx, |m, cx| m.join_invite(invite, cx));
        }
    }

    fn sign_out(&self, window: &mut Window, cx: &mut App) {
        let model = self.ctx.model.clone();
        let Some(settings) = model.read(cx).settings() else { return };
        let sheet = Sheet::new(format!("Sign out of {}?", settings.remote_url))
            .message(format!(
                "Its {} tracks, and the account's favourites, history, playlists and downloads, are removed from this device. Signing in again brings them back. Local files stay.",
                count(settings.remote_tracks)
            ))
            .action(
                Action::new("Sign Out", move |_, _, cx| {
                    model.update(cx, |m, cx| m.sign_out(cx))
                })
                .destructive(),
            );
        self.ctx.sheets.update(cx, |s, cx| s.show(sheet, window, cx));
    }

    fn keep(&mut self, choice: Keeping, window: &mut Window, cx: &mut Context<Self>) {
        let Some(settings) = self.ctx.model.read(cx).settings() else { return };
        let keeping = Keeping::of(&settings.cache_limit);
        if choice == Keeping::Limited {
            self.choosing_limit = keeping != Keeping::Limited;
            if self.choosing_limit {
                self.cache_limit.update(cx, |s, cx| s.focus(window, cx));
            }
            cx.notify();
            return;
        }
        // Back from a limit not yet typed, to what is already set.
        self.choosing_limit = false;
        cx.notify();
        if choice == keeping {
            return;
        }
        let stored = self.cache.read(cx).0;
        if choice == Keeping::Off && stored > 0 {
            self.confirm_limit("0".into(), stored, window, cx);
        } else {
            let limit = if choice == Keeping::Off { "0" } else { "" };
            self.set_limit(limit.into(), cx);
        }
    }

    fn set_limit(&mut self, limit: String, cx: &mut Context<Self>) {
        self.choosing_limit = false;
        self.ctx.model.update(cx, |m, cx| {
            if m.settings().is_some_and(|s| s.cache_limit != limit) {
                m.edit(cx, move |s| s.cache_limit = limit.clone());
            }
        });
    }

    /// Asks before downloads are deleted, saying how much is stored.
    fn confirm_limit(&self, limit: String, stored: u64, window: &mut Window, cx: &mut Context<Self>) {
        let title = if limit == "0" {
            "Keep only what plays next?".to_owned()
        } else {
            format!("Lower the limit to {limit}?")
        };
        let message = if limit == "0" {
            format!("{} of downloads are stored. Tracks fetched to play that are not in the queue ahead are deleted now. Downloads you asked for and favourite records stay.", bytes(stored))
        } else {
            format!("{} of downloads are stored. Those over the limit are deleted now, the tracks played longest ago first. Favourite records stay.", bytes(stored))
        };
        let pane = cx.entity();
        let sheet = Sheet::new(title).message(message).action(
            Action::new("Delete Downloads", move |_, _, cx| {
                let limit = limit.clone();
                pane.update(cx, |pane, cx| pane.set_limit(limit, cx))
            })
            .destructive(),
        );
        self.ctx.sheets.update(cx, |s, cx| s.show(sheet, window, cx));
    }

    fn commit_cache_limit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let typed = self.cache_limit.read(cx).value().to_string();
        let Some(settings) = self.ctx.model.read(cx).settings() else { return };
        if typed.trim() == cache::limit_gb(&settings.cache_limit) {
            return;
        }
        let current = settings.cache_limit.clone();
        match cache::parse_limit(&typed) {
            None => {}
            Some(Err(why)) => self.ctx.model.update(cx, |m, cx| m.report(why, cx)),
            Some(Ok(limit)) => {
                let stored = self.cache.read(cx).0;
                if cache::lowers(&limit, &current, stored) {
                    self.confirm_limit(limit, stored, window, cx);
                } else {
                    self.set_limit(limit, cx);
                }
            }
        }
    }

    fn connect_sections(
        &self,
        settings: &Settings,
        connection: &Option<ConnectionInfo>,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let model = self.ctx.model.read(cx);
        let entity = cx.entity();
        let mut sections = Vec::new();
        if settings.remote_signed_in {
            let server = settings.remote_url.clone();
            let mut rows = vec![
                FormRow::new("Server")
                    .control(copyable("copy-server", server, self.copied == Some("server"), {
                        let entity = entity.clone();
                        let server = settings.remote_url.clone();
                        move |cx| {
                            let server = server.clone();
                            entity.update(cx, |pane, cx| pane.copy("server", server, cx))
                        }
                    }, look))
                    .into_any_element(),
                FormRow::new("User")
                    .value(settings.remote_username.clone())
                    .into_any_element(),
                FormRow::new("Tracks")
                    .value(count(settings.remote_tracks))
                    .into_any_element(),
            ];
            if connection.as_ref().is_some_and(|c| c.sign_in_refused) {
                rows.push(note(
                    "Your server refused kōan's sign-in. Sign in again in Settings → Server.",
                    Icon::Warning,
                    Tone::Bad,
                    look,
                ));
            }
            let model_sync = self.ctx.model.clone();
            rows.push(buttons(
                vec![
                    // Only the syncs wait on the database writer. Signing
                    // out is a config write, and greying it out while a sync
                    // runs strands you on a server you are trying to leave.
                    Button::new("sync", ButtonKind::Bordered)
                        .label("Sync")
                        .icon(Icon::Sync)
                        .disabled(model.conflicts(Uses::REMOTE_TRACKS))
                        .on_click(move |_, _, cx| model_sync.update(cx, |m, cx| m.sync(cx)))
                        .into_any_element(),
                    Button::new("sign-out", ButtonKind::Bordered)
                        .label("Sign Out…")
                        .destructive(true)
                        .on_click({
                            let entity = entity.clone();
                            move |_, window, cx| entity.read(cx).sign_out(window, cx)
                        })
                        .into_any_element(),
                ],
                look,
            ));
            sections.push(section(Some("Signed in"), rows, None, look));
            sections.push(offers(connection, look));
        } else {
            let ready = [&self.url, &self.username, &self.secret]
                .iter()
                .all(|field| !field.read(cx).value().trim().is_empty());
            let secret_label = if self.with_key { "API key" } else { "Password" };
            let rows = vec![
                FormRow::new("Server URL")
                    .control(div().w(px(300.)).child(TextField::new(&self.url)))
                    .into_any_element(),
                FormRow::new("Username")
                    .control(div().w(px(300.)).child(TextField::new(&self.username)))
                    .into_any_element(),
                FormRow::new("Sign in with")
                    .control(
                        Segmented::new("with-key", ["Password", "API key"], self.with_key as usize)
                            .on_change({
                                let entity = entity.clone();
                                move |index, _, cx| {
                                    let with_key = *index == 1;
                                    entity.update(cx, |pane, cx| {
                                        pane.with_key = with_key;
                                        cx.notify();
                                    })
                                }
                            }),
                    )
                    .into_any_element(),
                FormRow::new(secret_label)
                    .control(div().w(px(300.)).child(TextField::new(&self.secret)))
                    .into_any_element(),
                buttons(
                    vec![
                        Button::new("sign-in", ButtonKind::Prominent)
                            .label("Sign In")
                            .disabled(!ready)
                            .on_click({
                                let entity = entity.clone();
                                move |_, window, cx| {
                                    entity.update(cx, |pane, cx| pane.sign_in(window, cx))
                                }
                            })
                            .into_any_element(),
                        Button::new("paste-invite", ButtonKind::Standard)
                            .label("Paste Invite")
                            .icon(Icon::Paste)
                            .on_click({
                                let entity = entity.clone();
                                move |_, window, cx| {
                                    let pasted = cx
                                        .read_from_clipboard()
                                        .and_then(|item| item.text())
                                        .unwrap_or_default();
                                    entity.update(cx, |pane, cx| pane.join(&pasted, false, window, cx))
                                }
                            })
                            .into_any_element(),
                    ],
                    look,
                ),
            ];
            sections.push(section(
                Some("Subsonic or Navidrome"),
                rows,
                Some("Paste an invite here, or into Server URL, and kōan fills in the rest. The account is checked against the server, then saved to config.local.toml, readable only by you.".into()),
                look,
            ));
        }

        let mut sync_rows = vec![{
            let model = self.ctx.model.clone();
            Toggle::new("auto-sync", "Keep the library in sync", settings.auto_sync)
                .on_change(move |on, _, cx| {
                    let on = *on;
                    model.update(cx, |m, cx| m.edit(cx, move |s| s.auto_sync = on))
                })
                .into_any_element()
        }];
        if settings.auto_sync {
            let model = self.ctx.model.clone();
            sync_rows.push(
                FormRow::new("Every")
                    .control(
                        Picker::new(
                            "sync-every",
                            SYNC_EVERY.map(|(label, _)| label),
                            SYNC_EVERY
                                .iter()
                                .position(|(_, mins)| *mins == settings.auto_sync_interval_mins),
                        )
                        .on_change(move |index, _, cx| {
                            let mins = SYNC_EVERY[*index].1;
                            model.update(cx, |m, cx| {
                                m.edit(cx, move |s| s.auto_sync_interval_mins = mins)
                            })
                        }),
                    )
                    .into_any_element(),
            );
        }
        sections.push(section(
            Some("Automatic sync"),
            sync_rows,
            Some("Each sync asks the server only for what changed since the last.".into()),
            look,
        ));
        sections.push(self.downloads(settings, look, cx));
        sections
    }

    fn downloads(&self, settings: &Settings, look: &Look, cx: &mut Context<Self>) -> AnyElement {
        let entity = cx.entity();
        let keeping = Keeping::of(&settings.cache_limit);
        let shown = if self.choosing_limit { Keeping::Limited } else { keeping };
        let ahead = cache::ahead_options(settings.download_ahead);
        let ahead_index = ahead.iter().position(|(_, v)| *v == settings.download_ahead);
        let stored = self.cache.read(cx).0;
        let clearing = self.ctx.model.read(cx).conflicts(Uses::DOWNLOADS);

        let workers = {
            let model = self.ctx.model.clone();
            Stepper::new("workers", "Parallel downloads", settings.download_workers as i64, 1..=16)
                .shown(settings.download_workers.to_string())
                .on_change(move |n, _, cx| {
                    let n = *n as u32;
                    model.update(cx, |m, cx| m.edit(cx, move |s| s.download_workers = n))
                })
        };
        let ahead_picker = {
            let model = self.ctx.model.clone();
            let values: Vec<Option<u32>> = ahead.iter().map(|(_, v)| *v).collect();
            Picker::new("ahead", ahead.iter().map(|(l, _)| l.as_str()), ahead_index).on_change(
                move |index, _, cx| {
                    let value = values[*index];
                    model.update(cx, |m, cx| m.edit(cx, move |s| s.download_ahead = value))
                },
            )
        };
        let keeping_picker = Picker::new(
            "keeping",
            Keeping::ALL.map(Keeping::label),
            Keeping::ALL.iter().position(|k| *k == shown),
        )
        .on_change({
            let entity = entity.clone();
            move |index, window, cx| {
                let choice = Keeping::ALL[*index];
                entity.update(cx, |pane, cx| pane.keep(choice, window, cx))
            }
        });

        let mut rows = vec![
            workers.into_any_element(),
            FormRow::new("Download ahead").control(ahead_picker).into_any_element(),
            FormRow::new("Keep downloads").control(keeping_picker).into_any_element(),
        ];
        if shown == Keeping::Limited {
            rows.push(
                FormRow::new("Limit in GB")
                    .control(div().w(px(120.)).child(TextField::new(&self.cache_limit)))
                    .into_any_element(),
            );
        }
        rows.push(
            FormRow::new("Using")
                .value(bytes(stored))
                .control(
                    Button::new("clear-cache", ButtonKind::Compact)
                        .label("Clear…")
                        .disabled(clearing || stored == 0)
                        .on_click(move |_, window, cx| {
                            entity.update(cx, |pane, cx| pane.confirm_clear(window, cx))
                        }),
                )
                .into_any_element(),
        );
        let note = cache::downloads_note(settings.download_ahead, keeping);
        section(
            Some("Downloads"),
            rows,
            (!note.is_empty()).then(|| note.into()),
            look,
        )
    }

    fn confirm_clear(&self, window: &mut Window, cx: &mut Context<Self>) {
        let stored = self.cache.read(cx).0;
        let model = self.ctx.model.clone();
        let sheet = Sheet::new("Clear downloads?")
            .message(format!(
                "Deletes the {} of tracks downloaded from the server. They are fetched again when they play.",
                bytes(stored)
            ))
            .action(
                Action::new("Clear Downloads", move |_, _, cx| {
                    model.update(cx, |m, cx| m.clear_cache(cx))
                })
                .destructive(),
            );
        self.ctx.sheets.update(cx, |s, cx| s.show(sheet, window, cx));
    }
}

/// A server's host, from its address.
pub fn host(url: &str) -> String {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split(['/', ':'])
        .next()
        .unwrap_or(url)
        .to_owned()
}

fn host_of(settings: &Settings) -> String {
    host(&settings.remote_url)
}

/// What the server said it is when koan signed in, and what that turns on.
fn offers(connection: &Option<ConnectionInfo>, look: &Look) -> AnyElement {
    let mut rows = Vec::new();
    match connection {
        Some(c) if c.server_kind.is_some() || c.open_subsonic => {
            let name = match c.server_kind.as_deref() {
                Some("koan") => "kōan".to_owned(),
                Some(kind) => {
                    let mut chars = kind.chars();
                    chars
                        .next()
                        .map(|first| first.to_uppercase().chain(chars).collect())
                        .unwrap_or_default()
                }
                None => "Subsonic".to_owned(),
            };
            let server = match &c.server_version {
                Some(version) => format!("{name} {version}"),
                None => name,
            };
            let devices = if !c.devices {
                "Not offered"
            } else if c.linked {
                "Connected"
            } else {
                "Offered, not connected"
            };
            rows.push(FormRow::new("Server").value(server).into_any_element());
            rows.push(
                FormRow::new("OpenSubsonic")
                    .value(if c.open_subsonic { "Yes" } else { "No" })
                    .into_any_element(),
            );
            rows.push(FormRow::new("Your devices").value(devices).into_any_element());
            for e in &c.extensions {
                let versions = e
                    .versions
                    .iter()
                    .map(|v| format!("v{v}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                rows.push(FormRow::data(e.name.clone()).value(versions).into_any_element());
            }
        }
        _ => rows.push(text("Not reached yet", TypeRole::Body, Tone::Muted, look).into_any_element()),
    }
    section(
        Some("What the server offers"),
        rows,
        Some("Asked when kōan signs in and whenever its link to the server reconnects. Features beyond Subsonic are used only where the server lists them.".into()),
        look,
    )
}

impl Render for ServerPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let Some(settings) = self.ctx.model.read(cx).settings().cloned() else {
            return page("server", &look, Vec::new());
        };
        let connection = self.ctx.connection.read(cx).0.clone();
        let entity = cx.entity();
        let sharing = self.ctx.sharing.read(cx);
        let status = mode_label(sharing, &connection);
        let mode = self.mode;
        let mut sections = vec![section(
            None,
            vec![
                FormRow::new("This device")
                    .control(
                        Picker::new(
                            "mode",
                            ["Connects to a server", "Shares its library"],
                            Some(mode as usize),
                        )
                        .on_change(move |index, _, cx| {
                            let mode = if *index == 1 { Mode::Share } else { Mode::Connect };
                            entity.update(cx, |pane, cx| {
                                pane.mode = mode;
                                cx.notify();
                            })
                        }),
                    )
                    .into_any_element(),
                status.render(&look),
            ],
            Some(
                if mode == Mode::Share {
                    "Your phone and other devices play this device's library, with an account you make here."
                } else {
                    "This device plays a kōan, Navidrome or Subsonic server's library, alongside its own folders."
                }
                .into(),
            ),
            &look,
        )];
        match mode {
            Mode::Share => sections.extend(self.share.sections(&self.ctx, &settings, &look, window, cx)),
            Mode::Connect if settings.share_enabled => sections.push(section(
                None,
                vec![text(
                    "This device is sharing its library. Turn sharing off under Shares its library to sign in to a server instead.",
                    TypeRole::Body,
                    Tone::Muted,
                    &look,
                )
                .into_any_element()],
                None,
                &look,
            )),
            Mode::Connect => sections.extend(self.connect_sections(&settings, &connection, &look, cx)),
        }
        page("server", &look, sections)
    }
}

/// What this device is doing about servers, as one line.
pub enum ModeLabel {
    Sharing(u16),
    NotSharing(SharedString),
    Connected(String),
    Own,
}

impl ModeLabel {
    pub fn render(&self, look: &Look) -> AnyElement {
        match self {
            ModeLabel::Sharing(port) => note(format!("Sharing on port {port}"), Icon::Server, Tone::Muted, look),
            ModeLabel::NotSharing(why) => note(format!("Not sharing: {why}"), Icon::Warning, Tone::Bad, look),
            ModeLabel::Connected(host) => note(format!("Connected to {host}"), Icon::Server, Tone::Muted, look),
            ModeLabel::Own => note("Playing this device's own library", Icon::Library, Tone::Muted, look),
        }
    }
}

pub fn mode_label(sharing: &crate::settings::sharing::Sharing, connection: &Option<ConnectionInfo>) -> ModeLabel {
    if let Some(serving) = &sharing.serving {
        ModeLabel::Sharing(serving.port)
    } else if let Some(failure) = &sharing.failure {
        ModeLabel::NotSharing(failure.clone())
    } else if let Some(server) = connection.as_ref().and_then(|c| c.signed_in_to.as_ref()) {
        ModeLabel::Connected(host(server))
    } else {
        ModeLabel::Own
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_host() {
        assert_eq!(host("https://music.example.com/"), "music.example.com");
        assert_eq!(host("http://10.0.0.2:4533"), "10.0.0.2");
        assert_eq!(host("music.local"), "music.local");
    }
}
