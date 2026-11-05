//! Server: what this device does about servers. It signs in to one, or, on
//! the desktop, is one, sharing its own library; and what it keeps of a
//! server's library.

use crate::components::{
    Button, ButtonKind, CopyText, Picker, Segmented, Stepper, TextField, Toggle,
};
use crate::theme::{Icon, Look, Tone, TypeRole, label};
use koan_kit::app::input::{InputEvent, InputState};
use koan_kit::app::*;
use koan_runtime::{ConnectionInfo, Settings};

use super::cache::{self, Keeping};
use crate::mirror::{Mirror, Slice};
use crate::settings::Ctx;
use crate::settings::form::{Row, buttons, note, page, section, text};
use crate::settings::model::{Uses, bytes, count};
use crate::settings::sheet::{Action, Sheet};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
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

pub struct ServerPane {
    pub(super) ctx: Ctx,
    /// The side the person chose to look at, while it differs from what
    /// this device is doing; otherwise the pane follows that.
    chosen: Option<Mode>,
    /// The side drawn last, so turning sharing off from the sharing side
    /// stays on it (`stay`).
    shown: Option<Mode>,
    /// One side only, without the choice between them or what is kept of
    /// a server's library: the setup's sign-in and sharing steps.
    pinned: Option<Mode>,
    url: Entity<InputState>,
    username: Entity<InputState>,
    secret: Entity<InputState>,
    with_key: bool,
    cache_limit: Entity<InputState>,
    /// "Up to a limit" chosen and no limit typed yet: nothing is written,
    /// and nothing deleted, until there is a number.
    choosing_limit: bool,
    /// The limit's own question is up: focus moving into it blurs the field,
    /// which must not ask again.
    asking_limit: bool,
    cache: Entity<Slice<u64>>,
    pub(super) share: super::share::ShareForm,
    _subscriptions: Vec<Subscription>,
}

impl ServerPane {
    pub fn new(ctx: Ctx, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = ctx.model.read(cx).settings().cloned();
        let field = |placeholder: &'static str,
                     secret: bool,
                     value: String,
                     window: &mut Window,
                     cx: &mut Context<Self>| {
            cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(crate::theme::label(placeholder))
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
            // The secret goes once it has signed in, and stays for another
            // try when it has not.
            cx.observe_in(&ctx.model, window, |pane, model, window, cx| {
                let signed_in = model
                    .read(cx)
                    .settings()
                    .is_some_and(|s| s.remote_signed_in);
                if signed_in && !pane.secret.read(cx).value().is_empty() {
                    pane.secret.update(cx, |s, cx| s.set_value("", window, cx));
                }
                cx.notify();
            }),
            cx.observe(&ctx.sharing, |_, _, cx| cx.notify()),
            cx.observe(&ctx.connection, |_, _, cx| cx.notify()),
            cx.observe(&cache, |_, _, cx| cx.notify()),
            // An invite, or an address with the account in it, pasted where
            // the address goes.
            cx.subscribe_in(
                &url,
                window,
                |pane, state, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Change) {
                        let typed = state.read(cx).value().to_string();
                        pane.join(&typed, true, window, cx);
                    }
                },
            ),
            cx.subscribe_in(&secret, window, |pane, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    pane.sign_in(cx);
                }
            }),
            cx.subscribe_in(
                &cache_limit,
                window,
                |pane, _, event: &InputEvent, window, cx| {
                    // Focus moving into the limit's own question blurs the
                    // field too: that question is the one being asked. Any
                    // other sheet takes the typed limit with it.
                    let commit = match event {
                        InputEvent::PressEnter { .. } => true,
                        InputEvent::Blur => !pane.asking_limit,
                        _ => false,
                    };
                    if commit {
                        pane.commit_cache_limit(window, cx);
                    }
                },
            ),
        ];
        Self {
            ctx,
            chosen: None,
            shown: None,
            pinned: None,
            url,
            username,
            secret,
            with_key: false,
            cache_limit,
            choosing_limit: false,
            asking_limit: false,
            cache,
            share,
            _subscriptions: subscriptions,
        }
    }

    /// Shows `mode`'s side alone, its essentials only.
    pub fn pinned(mut self, mode: Mode) -> Self {
        self.pinned = Some(mode);
        self
    }

    fn sign_in(&mut self, cx: &mut Context<Self>) {
        let url = self.url.read(cx).value().trim().to_owned();
        let username = self.username.read(cx).value().trim().to_owned();
        let secret = self.secret.read(cx).value().to_string();
        let signing_in = self
            .ctx
            .model
            .read(cx)
            .running()
            .any(|label| label.as_ref() == crate::settings::model::SIGNING_IN);
        if signing_in || url.is_empty() || username.is_empty() || secret.is_empty() {
            return;
        }
        let with_key = self.with_key;
        self.ctx
            .model
            .update(cx, |m, cx| m.sign_in(url, username, secret, with_key, cx));
    }

    /// Signs in through an invite, once the person says so: the question
    /// names the account and server, and what becomes of the sign-in it
    /// replaces. One for the account already signed in changes nothing.
    pub fn join(&mut self, text: &str, quiet: bool, window: &mut Window, cx: &mut Context<Self>) {
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
        let Some(settings) = model.read(cx).settings() else {
            return;
        };
        let same = settings.remote_url.trim_end_matches('/') == invite.server.trim_end_matches('/')
            && settings.remote_username == invite.username;
        // The account and server already signed in: nothing to do.
        if settings.remote_signed_in && same {
            model.update(cx, |m, cx| {
                m.done(
                    format!("You're already signed in as {} on {host}.", invite.username),
                    cx,
                )
            });
            return;
        }
        // Nothing changes until the person says so: a link can come from
        // anywhere, and signing in replaces whatever is signed in now.
        let message = if settings.remote_signed_in {
            let current = host_of(settings);
            format!(
                "kōan is signed in to {current}. Signing in here signs out of it: its tracks, favourites, history, playlists and downloads are removed from this device. Local files stay."
            )
        } else {
            format!(
                "kōan signs in to {host} as {} and brings its library to this device.",
                invite.username
            )
        };
        let title = format!("Sign in as {} on {host}?", invite.username);
        let join = {
            let model = model.clone();
            move |_: &[String], _: &mut Window, cx: &mut App| {
                let invite = invite.clone();
                model.update(cx, |m, cx| m.join_invite(invite, cx))
            }
        };
        let sheet = Sheet::new(title)
            .message(message)
            .action(Action::new("Sign In", join));
        self.ctx
            .sheets
            .update(cx, |s, cx| s.show(sheet, window, cx));
    }

    fn sign_out(&self, window: &mut Window, cx: &mut App) {
        let model = self.ctx.model.clone();
        let Some(settings) = model.read(cx).settings() else {
            return;
        };
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
        self.ctx
            .sheets
            .update(cx, |s, cx| s.show(sheet, window, cx));
    }

    fn keep(&mut self, choice: Keeping, window: &mut Window, cx: &mut Context<Self>) {
        let Some(settings) = self.ctx.model.read(cx).settings() else {
            return;
        };
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
    fn confirm_limit(
        &mut self,
        limit: String,
        stored: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = if limit == "0" {
            "Keep only what plays next?".to_owned()
        } else {
            format!("Lower the limit to {limit}?")
        };
        let message = if limit == "0" {
            format!(
                "{} of downloads are stored. Tracks fetched to play that are not in the queue ahead are deleted now. Downloads you asked for and favourite records stay.",
                bytes(stored)
            )
        } else {
            format!(
                "{} of downloads are stored. Those over the limit are deleted now, the tracks played longest ago first. Favourite records stay.",
                bytes(stored)
            )
        };
        let pane = cx.entity();
        let kept = pane.clone();
        let sheet = Sheet::new(title)
            .message(message)
            // Cancelled, the field shows the limit kept, so leaving it does
            // not ask again.
            .dismiss_with("Cancel", move |window, cx| {
                kept.update(cx, |pane, cx| {
                    pane.asking_limit = false;
                    pane.show_stored_limit(window, cx)
                })
            })
            .action(
                Action::new("Delete Downloads", move |_, _, cx| {
                    let limit = limit.clone();
                    pane.update(cx, |pane, cx| {
                        pane.asking_limit = false;
                        pane.set_limit(limit, cx)
                    })
                })
                .destructive(),
            );
        self.asking_limit = true;
        self.ctx
            .sheets
            .update(cx, |s, cx| s.show(sheet, window, cx));
    }

    /// The field back to the limit stored.
    fn show_stored_limit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(settings) = self.ctx.model.read(cx).settings() else {
            return;
        };
        let stored = cache::limit_gb(&settings.cache_limit);
        self.cache_limit
            .update(cx, |s, cx| s.set_value(stored, window, cx));
    }

    fn commit_cache_limit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let typed = self.cache_limit.read(cx).value().to_string();
        let Some(settings) = self.ctx.model.read(cx).settings() else {
            return;
        };
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
                Row::new("Server")
                    .control(CopyText::new("copy-server", server).help("Copy the server's address"))
                    .into_any_element(),
                Row::new("User")
                    .value(settings.remote_username.clone())
                    .into_any_element(),
                Row::new("Tracks")
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
                            move |_, window, cx| {
                                entity.update(cx, |this, cx| this.sign_out(window, cx))
                            }
                        })
                        .into_any_element(),
                ],
                look,
            ));
            sections.push(section(Some("Signed in"), rows, None, look));
            if self.pinned.is_some() {
                return sections;
            }
            sections.push(offers(connection, look));
        } else {
            let signing_in = model
                .running()
                .any(|label| label.as_ref() == crate::settings::model::SIGNING_IN);
            let ready = !signing_in
                && [&self.url, &self.username, &self.secret]
                    .iter()
                    .all(|field| !field.read(cx).value().trim().is_empty());
            let secret_label = if self.with_key { "API key" } else { "Password" };
            let rows = vec![
                Row::new("Server URL")
                    .control(div().w_full().child(TextField::new(&self.url)))
                    .into_any_element(),
                Row::new("Username")
                    .control(div().w_full().child(TextField::new(&self.username)))
                    .into_any_element(),
                Row::new("Sign in with")
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
                Row::new(secret_label)
                    .control(div().w_full().child(TextField::new(&self.secret)))
                    .into_any_element(),
                buttons(
                    vec![
                        Button::new("sign-in", ButtonKind::Prominent)
                            .label("Sign In")
                            .disabled(!ready)
                            .on_click({
                                let entity = entity.clone();
                                move |_, _, cx| entity.update(cx, |pane, cx| pane.sign_in(cx))
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
                                    entity.update(cx, |pane, cx| {
                                        pane.join(&pasted, false, window, cx)
                                    })
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
        if self.pinned.is_some() {
            return sections;
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
                Row::new("Every")
                    .control(
                        Picker::new(
                            "sync-every",
                            SYNC_EVERY.map(|(label, _)| label),
                            SYNC_EVERY
                                .iter()
                                .position(|(_, mins)| *mins == settings.auto_sync_interval_mins),
                        )
                        .named("sync every")
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
        sections.push(section(
            Some("Covers"),
            vec![{
                let model = self.ctx.model.clone();
                Toggle::new(
                    "preload-covers",
                    "Fetch every record's cover ahead",
                    settings.preload_covers,
                )
                .on_change(move |on, _, cx| {
                    let on = *on;
                    model.update(cx, |m, cx| m.edit(cx, move |s| s.preload_covers = on))
                })
                .into_any_element()
            }],
            Some("Covers are fetched in the background, after what is on screen, so browsing never waits for one. They are kept with the downloads and count against the same limit. Turning this off removes nothing already fetched.".into()),
            look,
        ));
        sections.push(self.downloads(settings, look, cx));
        sections
    }

    fn downloads(&self, settings: &Settings, look: &Look, cx: &mut Context<Self>) -> AnyElement {
        let entity = cx.entity();
        let keeping = Keeping::of(&settings.cache_limit);
        let shown = if self.choosing_limit {
            Keeping::Limited
        } else {
            keeping
        };
        let ahead = cache::ahead_options(settings.download_ahead);
        let ahead_index = ahead
            .iter()
            .position(|(_, v)| *v == settings.download_ahead);
        let stored = self.cache.read(cx).0;
        let clearing = self.ctx.model.read(cx).conflicts(Uses::DOWNLOADS);

        let workers = {
            let model = self.ctx.model.clone();
            Stepper::new(
                "workers",
                "Parallel downloads",
                settings.download_workers as i64,
                1..=16,
            )
            .shown(settings.download_workers.to_string())
            .on_change(move |n, _, cx| {
                let n = *n as u32;
                model.update(cx, |m, cx| m.edit(cx, move |s| s.download_workers = n))
            })
        };
        let ahead_picker = {
            let model = self.ctx.model.clone();
            let values: Vec<Option<u32>> = ahead.iter().map(|(_, v)| *v).collect();
            Picker::new("ahead", ahead.iter().map(|(l, _)| l.as_str()), ahead_index)
                .named("download ahead")
                .on_change(move |index, _, cx| {
                    let value = values[*index];
                    model.update(cx, |m, cx| m.edit(cx, move |s| s.download_ahead = value))
                })
        };
        let keeping_picker = Picker::new(
            "keeping",
            Keeping::ALL.map(Keeping::label),
            Keeping::ALL.iter().position(|k| *k == shown),
        )
        .named("keep downloads")
        .on_change({
            let entity = entity.clone();
            move |index, window, cx| {
                let choice = Keeping::ALL[*index];
                entity.update(cx, |pane, cx| pane.keep(choice, window, cx))
            }
        });

        let mut rows = vec![
            workers.into_any_element(),
            Row::new("Download ahead")
                .control(ahead_picker)
                .into_any_element(),
            Row::new("Keep downloads")
                .control(keeping_picker)
                .into_any_element(),
        ];
        if shown == Keeping::Limited {
            rows.push(
                Row::new("Limit in GB")
                    .control(
                        div()
                            .w(look.sized(120.0))
                            .child(TextField::new(&self.cache_limit)),
                    )
                    .into_any_element(),
            );
        }
        rows.push(
            Row::new("Using")
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
        self.ctx
            .sheets
            .update(cx, |s, cx| s.show(sheet, window, cx));
    }
}

/// A server's host, from its address.
pub fn host(url: &str) -> String {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let authority = rest.split('/').next().unwrap_or(rest);
    // An IPv6 literal is bracketed, and its colons are not a port's.
    if authority.starts_with('[') {
        return authority
            .split_inclusive(']')
            .next()
            .unwrap_or(authority)
            .to_owned();
    }
    authority.split(':').next().unwrap_or(authority).to_owned()
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
            rows.push(Row::new("Server").value(server).into_any_element());
            rows.push(
                Row::new("OpenSubsonic")
                    .value(if c.open_subsonic { "Yes" } else { "No" })
                    .into_any_element(),
            );
            rows.push(Row::new("Your devices").value(devices).into_any_element());
            for e in &c.extensions {
                let versions = e
                    .versions
                    .iter()
                    .map(|v| format!("v{v}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                rows.push(Row::data(e.name.clone()).value(versions).into_any_element());
            }
        }
        _ => {
            rows.push(text("Not reached yet", TypeRole::Body, Tone::Muted, look).into_any_element())
        }
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
        let doing = if settings.share_enabled || sharing.serving.is_some() {
            Mode::Share
        } else {
            Mode::Connect
        };
        self.chosen = stay(self.shown, self.chosen, doing);
        let mode = self.pinned.or(self.chosen).unwrap_or(doing);
        self.shown = Some(mode);
        let mut sections = Vec::new();
        if self.pinned == Some(Mode::Share) {
            sections.push(section(None, vec![status.render(&look)], None, &look));
        } else if self.pinned.is_none() {
            sections.push(self.mode_section(mode, doing, &status, &look, entity.clone()));
        }
        match mode {
            Mode::Share => sections.extend(self.share.sections(&self.ctx, &settings, &look, cx)),
            Mode::Connect if settings.share_enabled => {
                let mut rows = vec![text(
                    if self.pinned.is_some() {
                        "This device is sharing its library. It signs in to a server once it stops sharing."
                    } else {
                        "This device is sharing its library. Turn sharing off under Shares its library to sign in to a server instead."
                    },
                    TypeRole::Body,
                    Tone::Muted,
                    &look,
                )
                .into_any_element()];
                if self.pinned.is_some() {
                    let ctx = self.ctx.clone();
                    let busy = self.ctx.sharing.read(cx).busy();
                    rows.push(buttons(
                        vec![
                            Button::new("stop-sharing", ButtonKind::Bordered)
                                .label("Stop Sharing…")
                                .disabled(busy)
                                .on_click(move |_, window, cx| {
                                    super::share::ShareForm::ask_stop_sharing(
                                        &ctx,
                                        window,
                                        cx,
                                        |_, _| {},
                                    )
                                })
                                .into_any_element(),
                        ],
                        &look,
                    ));
                }
                sections.push(section(None, rows, None, &look));
            }
            Mode::Connect => {
                sections.extend(self.connect_sections(&settings, &connection, &look, cx))
            }
        }
        page("server", &look, sections)
    }
}

impl ServerPane {
    /// The choice between signing in and sharing, with what this device
    /// does now under it.
    fn mode_section(
        &self,
        mode: Mode,
        doing: Mode,
        status: &ModeLabel,
        look: &Look,
        entity: Entity<Self>,
    ) -> AnyElement {
        section(
            None,
            vec![
                Row::new("This device")
                    .control(
                        Picker::new(
                            "mode",
                            ["Connects to a server", "Shares its library"],
                            Some(mode as usize),
                        )
                        .named("this device")
                        .on_change(move |index, _, cx| {
                            let mode = if *index == 1 { Mode::Share } else { Mode::Connect };
                            entity.update(cx, |pane, cx| {
                                pane.chosen = (mode != doing).then_some(mode);
                                cx.notify();
                            })
                        }),
                    )
                    .into_any_element(),
                status.render(look),
            ],
            Some(
                if mode == Mode::Share {
                    "Your phone and other devices play this device's library, with an account you make here."
                } else {
                    "This device plays a kōan, Navidrome or Subsonic server's library, alongside its own folders."
                }
                .into(),
            ),
            look,
        )
    }
}

/// The side chosen, given the side drawn last and what the device does now.
/// Sharing turned off while its side shows leaves that side showing, with
/// sharing off, rather than sending the pane to the sign-in form.
fn stay(shown: Option<Mode>, chosen: Option<Mode>, doing: Mode) -> Option<Mode> {
    match (shown, chosen, doing) {
        (Some(Mode::Share), None, Mode::Connect) => Some(Mode::Share),
        _ => chosen,
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
    /// What it says, in the theme's case, with its glyph and tone: the same
    /// words in Settings and at the foot of the sidebar.
    pub fn line(&self) -> (Icon, String, Tone) {
        match self {
            ModeLabel::Sharing(port) => (
                Icon::Server,
                label(format!("Sharing on port {port}")).to_string(),
                Tone::Muted,
            ),
            ModeLabel::NotSharing(why) => (
                Icon::Warning,
                format!("{}{why}", label("Not sharing: ")),
                Tone::Bad,
            ),
            ModeLabel::Connected(host) => (
                Icon::Server,
                format!("{}{host}", label("Connected to ")),
                Tone::Muted,
            ),
            ModeLabel::Own => (
                Icon::Library,
                label("Playing this device's own library").to_string(),
                Tone::Muted,
            ),
        }
    }

    pub fn render(&self, look: &Look) -> AnyElement {
        let (icon, text, tone) = self.line();
        note(text, icon, tone, look)
    }
}

pub fn mode_label(
    sharing: &crate::settings::sharing::Sharing,
    connection: &Option<ConnectionInfo>,
) -> ModeLabel {
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
    use super::{Mode, host, stay};

    #[test]
    fn turning_sharing_off_stays_on_the_sharing_side() {
        // Sharing, then turned off: the sharing side stays, sharing off.
        assert_eq!(
            stay(Some(Mode::Share), None, Mode::Connect),
            Some(Mode::Share)
        );
        // Opened while not sharing: the sign-in side, as before.
        assert_eq!(stay(None, None, Mode::Connect), None);
        assert_eq!(stay(Some(Mode::Connect), None, Mode::Connect), None);
        // Sharing: the pane follows it.
        assert_eq!(stay(Some(Mode::Share), None, Mode::Share), None);
    }

    #[test]
    fn reads_a_host() {
        assert_eq!(host("https://music.example.com/"), "music.example.com");
        assert_eq!(host("http://10.0.0.2:4533"), "10.0.0.2");
        assert_eq!(host("music.local"), "music.local");
        assert_eq!(host("http://[fe80::1]:4533/rest"), "[fe80::1]");
    }
}
