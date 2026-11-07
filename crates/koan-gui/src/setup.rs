//! The setup guide: what kōan is, where the music is, sharing it with the
//! person's other devices and reaching them away from home, playback, and
//! the keys worth knowing. A window of its own, so it stands without the
//! main window's state behind it.
//!
//! It opens by itself on a launch with nothing set up (no folders, no
//! server, not sharing) until it has been seen once, which the
//! `desktop-setup` file in the state directory records. Settings → Library
//! opens it any time, as does the kōan menu, and a reset to defaults opens
//! it again. Its steps reuse the Settings panes, on the app's one settings
//! model, so what it sets is what Settings shows.

use std::path::PathBuf;

use koan_kit::app::*;
use koan_runtime::InviteTo;

use crate::components::{
    Badge, Button, ButtonKind, Check, CheckItem, ChoiceCards, Steps, close_button,
    draws_decorations, moves_window,
};
use crate::mirror::Mirror;
use crate::platform::actions::{self, caption};
use crate::settings::form::{Row, buttons, item, page, section, text};
use crate::settings::panes::library::{choose_folders, homely};
use crate::settings::panes::playback::PlaybackPane;
use crate::settings::panes::server::{Mode, ServerPane};
use crate::settings::panes::share::ShareForm;
use crate::settings::sharing::Landed;
use crate::settings::{Ctx, Settings, Sheets};
use crate::theme::{self, Icon, Look, Space, Tone, TypeRole, label};

/// Where Tailscale is downloaded, for every platform.
const TAILSCALE_DOWNLOAD: &str = "https://tailscale.com/download";
/// The Mac app, opened from the checklist when it is installed and stopped.
const TAILSCALE_APP: &str = "/Applications/Tailscale.app";

/// The file that records the guide has been seen, in the state directory.
const SEEN: &str = "desktop-setup";

fn seen_path() -> PathBuf {
    koan_core::places::state().join(SEEN)
}

/// The guide's window and view while it is open.
#[derive(Default)]
struct Slot(Option<(AnyWindowHandle, WeakEntity<SetupWindow>)>);

impl Global for Slot {}

/// Opens the guide when nothing is set up and it has not been seen: no
/// folders, no server signed in to, not sharing. A launch that already
/// opened Settings (an invite or a pairing link) is in the middle of
/// something else and is left to it. Someone who set kōan up some other way
/// is recorded as having seen it, so removing everything later does not
/// bring it back.
pub fn offer_at_launch(cx: &mut App) {
    let model = cx.global::<Settings>().model.clone();
    model.update(cx, |model, cx| {
        model.when_loaded(cx, |cx| {
            let seen = cx
                .background_executor()
                .spawn(async { seen_path().exists() });
            cx.spawn(async move |cx| {
                if seen.await {
                    return;
                }
                cx.update(|cx| {
                    let settings = cx.global::<Settings>();
                    if settings.is_open() {
                        return;
                    }
                    let set_up = settings.model.read(cx).settings().is_some_and(|s| {
                        !s.library_folders.is_empty() || s.remote_signed_in || s.share_enabled
                    });
                    if set_up {
                        mark_seen(cx);
                    } else {
                        open(cx);
                    }
                });
            })
            .detach();
        })
    });
}

/// Opens the guide, or brings the open one forward.
pub fn open(cx: &mut App) {
    if let Some((handle, _)) = cx.try_global::<Slot>().and_then(|slot| slot.0.clone())
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return;
    }
    let opened = crate::components::SecondaryWindow::new("Set Up kōan", crate::app::APP_ID)
        .size(760.0, 760.0)
        .min(560.0, 560.0)
        .transparent_titlebar()
        .open(cx, |window, cx| cx.new(|cx| SetupWindow::new(window, cx)));
    match opened {
        Ok((handle, view)) => {
            cx.set_global(Slot(Some((handle, view.downgrade()))));
            crate::platform::residency::settle_dock(cx);
        }
        Err(e) => log::error!("the setup window did not open: {e}"),
    }
}

/// Closes the guide, as seen, when Settings takes over: an invite or a
/// pairing link opened it on a pane of its own.
pub fn close(cx: &mut App) {
    let Some((handle, view)) = cx.try_global::<Slot>().and_then(|slot| slot.0.clone()) else {
        return;
    };
    handle
        .update(cx, |_, window, cx| {
            view.update(cx, |setup, cx| setup.close(window, cx)).ok();
        })
        .ok();
}

/// What a reset to defaults ends with: the app drawn as the defaults say,
/// the library's filters cleared, the guide unseen and opened again.
pub fn after_reset(cx: &mut App) {
    let engine = crate::engine::engine(cx);
    crate::settings::apply_appearance(engine, cx);
    crate::rainbow::reload(cx);
    // Opening at login is the system's to keep, not the file's.
    use crate::platform::residency::{self, Login};
    if matches!(residency::login(cx), Login::On | Login::NeedsApproval) {
        residency::set_login(false, cx);
    }
    // With the main window closed its filters are only on disk, which the
    // next window reads; without the file it starts from the defaults.
    let browse = crate::app::main_shell(cx)
        .and_then(|shell| shell.read(cx).library().cloned())
        .map(|stage| stage.read(cx).browse().clone());
    let cleared = browse.is_some();
    if let Some(browse) = browse {
        browse.update(cx, |browse, cx| browse.clear(cx));
    }
    let forget = cx.background_executor().spawn(async move {
        if !cleared {
            crate::library::browse::forget_stored();
        }
        match std::fs::remove_file(seen_path()) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => log::warn!("setup: could not forget it was seen: {e}"),
        }
    });
    // A guide already open starts again from the beginning.
    if let Some((handle, view)) = cx.try_global::<Slot>().and_then(|slot| slot.0.clone()) {
        handle
            .update(cx, |_, window, cx| {
                view.update(cx, |setup, cx| setup.start_over(window, cx))
                    .ok();
            })
            .ok();
    }
    cx.spawn(async move |cx| {
        forget.await;
        cx.update(open);
    })
    .detach();
}

/// Records the guide as seen, off the main thread.
fn mark_seen(cx: &mut App) {
    cx.background_executor()
        .spawn(async {
            let path = seen_path();
            let written = path
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::write(&path, "seen\n"));
            if let Err(e) = written {
                log::warn!("setup: could not record it was seen: {e}");
            }
        })
        .detach();
}

/// Where the person's music is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Music {
    Here,
    Server,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Welcome,
    Folders,
    Share,
    Away,
    Server,
    Playback,
    Keys,
}

impl Step {
    fn name(self) -> &'static str {
        match self {
            Step::Welcome => "Welcome",
            Step::Folders => "Your music",
            Step::Share => "Sharing",
            Step::Away => "Away from home",
            Step::Server => "Your server",
            Step::Playback => "Playback",
            Step::Keys => "Finding your way",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Step::Welcome => "Welcome to kōan",
            Step::Folders => "Where your music is",
            Step::Share => "Your other devices",
            Step::Away => "Away from home",
            Step::Server => "Sign in to your server",
            Step::Playback => "Playback",
            Step::Keys => "Finding your way",
        }
    }

    fn intro(self) -> &'static str {
        match self {
            Step::Welcome => {
                "kōan plays your music as it was made: bit-perfect and gapless, with your output switched to each track's own sample rate, so nothing is resampled on the way to your speakers. It plays the files on this computer, a server's library, or both, and your phone and other devices can play along.\n\nThis guide sets up the essentials. Everything in it can be changed later in Settings."
            }
            Step::Folders => {
                "Add the folders your music is in. kōan reads each file's tags to build the library, and watches the folders, so music you add later appears by itself. Your files are never moved or changed unless you organise them."
            }
            Step::Share => {
                "This computer can be the server for your other devices: your phone, a tablet, the TV. They sign in with an account you make here, and play its library over your network."
            }
            Step::Away => {
                "At home, your devices find this computer on the network by themselves. Away from home they need a way in. Tailscale is the simplest: a private network between your own devices, free for personal use, with nothing opened to the internet."
            }
            Step::Server => {
                "Sign in to a kōan, Navidrome or Subsonic server, or paste an invite someone sent you. kōan plays the server's library and keeps what you play on this computer, so it carries on when the server cannot be reached."
            }
            Step::Playback => {
                "The output kōan plays through, how pause sounds, and loudness. The rest is in Settings → Playback."
            }
            Step::Keys => {
                "kōan is quickest from the keyboard. These are the keys worth learning first; single keys work anywhere outside a text field."
            }
        }
    }
}

/// The steps for where the music is and whether it is shared.
fn steps(music: Option<Music>, share: Option<bool>) -> Vec<Step> {
    let mut steps = vec![Step::Welcome];
    match music {
        Some(Music::Server) => steps.push(Step::Server),
        _ => {
            steps.extend([Step::Folders, Step::Share]);
            if share == Some(true) {
                steps.push(Step::Away);
            }
        }
    }
    steps.extend([Step::Playback, Step::Keys]);
    steps
}

/// The keys the last step lists, each with what it does and the keys that
/// do it, read from the shortcut tables once.
fn keys_worth_knowing() -> &'static [(&'static str, Vec<&'static str>)] {
    use actions::*;
    static KEYS: std::sync::OnceLock<Vec<(&'static str, Vec<&'static str>)>> =
        std::sync::OnceLock::new();
    KEYS.get_or_init(|| {
        let actions: [(&'static str, Box<dyn Action>); 10] = [
            ("Search", Box::new(Search)),
            ("Find and play", Box::new(AddMusic)),
            ("Play or pause", Box::new(bare::TogglePlayPause)),
            ("Next track", Box::new(Next)),
            ("Previous track", Box::new(Previous)),
            ("Favourite the track", Box::new(FavouriteCurrentTrack)),
            ("Lyrics", Box::new(ToggleLyrics)),
            ("EQ", Box::new(ShowEq)),
            ("Settings", Box::new(Settings)),
            ("Every shortcut", Box::new(ShowShortcuts)),
        ];
        actions
            .into_iter()
            .map(|(what, action)| (what, keys_for(action.as_ref())))
            .collect()
    })
}

pub struct SetupWindow {
    ctx: Ctx,
    music: Option<Music>,
    share: Option<bool>,
    step: Step,
    /// The Settings panes the steps show, each made on first arriving at
    /// its step and kept, so going back and on keeps what was typed.
    panes: Vec<(Step, AnyView)>,
    /// Whether Tailscale is installed, running or not: `None` until read.
    tailscale_installed: Option<bool>,
    /// Whether the Mac app is installed, rather than only the command line
    /// (Homebrew's formula), which the checklist cannot open.
    tailscale_app: bool,
    reading: Option<Task<()>>,
    choosing: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl SetupWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = cx.global::<Settings>();
        let ctx = Ctx {
            model: settings.model.clone(),
            sharing: settings.sharing.clone(),
            sheets: cx.new(|_| Sheets::default()),
            connection: cx.global::<Mirror>().connection.clone(),
        };
        // Opened again later, it starts from what is set up.
        let (music, share) = match ctx.model.read(cx).settings() {
            Some(s) if s.remote_signed_in => (Some(Music::Server), None),
            Some(s) if s.share_enabled => (Some(Music::Here), Some(true)),
            Some(s) if !s.library_folders.is_empty() => (Some(Music::Here), Some(false)),
            _ => (None, None),
        };
        let subscriptions = vec![
            theme::follow_appearance(window, cx),
            theme::observe_accent_then(cx, |_: &mut Self, cx| cx.notify()),
            cx.observe(&ctx.model, |_, _, cx| cx.notify()),
            cx.observe(&ctx.sharing, |_, _, cx| cx.notify()),
            cx.observe(&ctx.sheets, |_, _, cx| cx.notify()),
            // Coming back from installing or starting Tailscale, or from
            // the terminal, is the moment to look again.
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() {
                    this.ctx.model.update(cx, |model, cx| model.reload(cx));
                    if this.step == Step::Away {
                        this.read_tailscale(cx);
                    }
                }
            }),
            // Only this window's: one opened after it, before this one was
            // let go of, stays in the slot.
            cx.on_release({
                let id = window.window_handle().window_id();
                move |_, cx| {
                    if let Some(slot) = cx.try_global::<Slot>()
                        && slot.0.as_ref().is_some_and(|(h, _)| h.window_id() == id)
                    {
                        cx.global_mut::<Slot>().0 = None;
                    }
                }
            }),
        ];
        window.on_window_should_close(cx, {
            let sheets = ctx.sheets.clone();
            move |window, cx| {
                crate::settings::hand_on_unanswered(&sheets, window, cx);
                mark_seen(cx);
                true
            }
        });
        Self {
            ctx,
            music,
            share,
            step: Step::Welcome,
            panes: Vec::new(),
            tailscale_installed: None,
            tailscale_app: false,
            reading: None,
            choosing: None,
            _subscriptions: subscriptions,
        }
    }

    fn steps(&self) -> Vec<Step> {
        steps(self.music, self.share)
    }

    /// Back to the first step with nothing chosen, as after a reset. A
    /// question left open is answered as its Cancel would be, since it was
    /// about what the reset has changed.
    fn start_over(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        crate::settings::hand_on_unanswered(&self.ctx.sheets, window, cx);
        self.music = None;
        self.share = None;
        self.step = Step::Welcome;
        self.panes.clear();
        cx.notify();
    }

    /// Whether the step showing has what it needs to go on from.
    fn ready(&self) -> bool {
        match self.step {
            Step::Welcome => self.music.is_some(),
            Step::Share => self.share.is_some(),
            _ => true,
        }
    }

    /// The pane the step showing draws, if it draws one.
    fn pane(&self) -> Option<AnyView> {
        let wanted = match self.step {
            Step::Share => self.share == Some(true),
            Step::Server | Step::Playback => true,
            _ => false,
        };
        wanted
            .then(|| self.panes.iter().find(|(s, _)| *s == self.step))
            .flatten()
            .map(|(_, view)| view.clone())
    }

    fn go(&mut self, step: Step, window: &mut Window, cx: &mut Context<Self>) {
        self.step = step;
        let ctx = self.ctx.clone();
        let made = self.panes.iter().any(|(s, _)| *s == step);
        let pane: Option<AnyView> = match step {
            _ if made => None,
            Step::Share if self.share == Some(true) => Some(
                cx.new(|cx| ServerPane::new(ctx, window, cx).pinned(Mode::Share))
                    .into(),
            ),
            Step::Server => Some(
                cx.new(|cx| ServerPane::new(ctx, window, cx).pinned(Mode::Connect))
                    .into(),
            ),
            Step::Playback => Some(cx.new(|cx| PlaybackPane::new(ctx, cx)).into()),
            _ => None,
        };
        if let Some(pane) = pane {
            self.panes.push((step, pane));
        }
        if step == Step::Away {
            self.read_tailscale(cx);
        }
        cx.notify();
    }

    /// Makes the choices a person would, for tools that draw the guide
    /// offscreen: music here (and shared or not) or on a server.
    #[doc(hidden)]
    pub fn choose(&mut self, here: bool, share: bool) {
        self.music = Some(if here { Music::Here } else { Music::Server });
        self.share = here.then_some(share);
    }

    /// The step showing, by its name.
    pub fn step(&self) -> &'static str {
        self.step.name()
    }

    /// On to the next step, or done after the last.
    pub fn next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let steps = self.steps();
        match steps.iter().position(|s| *s == self.step) {
            Some(at) if at + 1 < steps.len() => self.go(steps[at + 1], window, cx),
            _ => self.finish(window, cx),
        }
    }

    fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let steps = self.steps();
        if let Some(at) = steps.iter().position(|s| *s == self.step)
            && at > 0
        {
            self.go(steps[at - 1], window, cx);
        }
    }

    /// Done or skipped: seen, the window goes, and the main window comes
    /// forward.
    fn finish(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close(window, cx);
        crate::app::show_window(cx);
    }

    /// Seen, and the window goes, its sheets answered as their Cancel would.
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        crate::settings::hand_on_unanswered(&self.ctx.sheets, window, cx);
        mark_seen(cx);
        window.remove_window();
    }

    /// Sharing or not. Not sharing, while this computer shares, is the
    /// choice once the person has said to stop.
    fn choose_sharing(&mut self, share: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.share == Some(share) {
            return;
        }
        let sharing = self
            .ctx
            .model
            .read(cx)
            .settings()
            .is_some_and(|s| s.share_enabled);
        if !share && sharing {
            let this = cx.entity().downgrade();
            ShareForm::ask_stop_sharing(&self.ctx, window, cx, move |window, cx| {
                this.update(cx, |this, cx| {
                    this.share = Some(false);
                    this.go(Step::Share, window, cx);
                })
                .ok();
            });
            return;
        }
        self.share = Some(share);
        self.go(Step::Share, window, cx);
    }

    /// Reads whether Tailscale is installed, and has Sharing read whether it
    /// is running. Both run a process or touch the disk, off the main thread.
    fn read_tailscale(&mut self, cx: &mut Context<Self>) {
        self.ctx
            .sharing
            .update(cx, |sharing, cx| sharing.refresh(cx));
        let engine = self.ctx.model.read(cx).engine().clone();
        self.reading = Some(cx.spawn(async move |this, cx| {
            let installed = engine.tailscale_installed().await;
            let app = cx
                .background_executor()
                .spawn(async { std::path::Path::new(TAILSCALE_APP).exists() })
                .await;
            this.update(cx, |this, cx| {
                this.tailscale_installed = Some(installed);
                this.tailscale_app = app;
                cx.notify();
            })
            .ok();
        }));
    }

    fn body(&mut self, look: &Look, cx: &mut Context<Self>) -> AnyElement {
        if let Some(pane) = self.pane() {
            if self.step != Step::Share {
                return pane.into_any_element();
            }
            // The question stays above the sharing pane, which scrolls.
            return div()
                .size_full()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex_none()
                        .px(look.space(Space::Xl))
                        .pt(look.space(Space::Xl))
                        .child(self.share_question(look, cx)),
                )
                .child(div().flex_1().min_h_0().child(pane))
                .into_any_element();
        }
        let sections = match self.step {
            Step::Welcome => vec![self.welcome(look, cx)],
            Step::Folders => self.folders(look, cx),
            Step::Share => vec![self.share_question(look, cx)],
            Step::Away => self.away(look, cx),
            Step::Keys => self.keys(look, cx),
            Step::Server | Step::Playback => Vec::new(),
        };
        page("setup", look, sections).into_any_element()
    }

    fn welcome(&self, look: &Look, cx: &mut Context<Self>) -> AnyElement {
        let entity = cx.entity();
        let chosen = self.music.map(|m| m as usize);
        section(
            Some("Where is your music?"),
            vec![
                ChoiceCards::new("setup-music", chosen)
                    .named("where is your music")
                    .card(
                        Icon::Folder,
                        "On this computer",
                        Some("Folders of music files here, or on a drive or share this computer reaches. This computer can then share them with your other devices."),
                    )
                    .card(
                        Icon::Server,
                        "On a server",
                        Some("A kōan, Navidrome or Subsonic server you or someone else runs. This computer plays its library, and can add folders of its own beside it."),
                    )
                    .on_change(move |index, _, cx| {
                        let music = if *index == 0 { Music::Here } else { Music::Server };
                        entity.update(cx, |this, cx| {
                            this.music = Some(music);
                            cx.notify();
                        })
                    })
                    .into_any_element(),
            ],
            None,
            look,
        )
    }

    fn folders(&mut self, look: &Look, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let model = self.ctx.model.read(cx);
        let Some(settings) = model.settings() else {
            return Vec::new();
        };
        let scanning = model.conflicts(crate::settings::model::Uses::LOCAL_LIBRARY);
        let entity = cx.entity();
        let mut rows: Vec<AnyElement> = settings
            .library_folders
            .iter()
            .enumerate()
            .map(|(index, folder)| {
                let (path, tracks) = (folder.path.clone(), folder.tracks);
                let ctx = self.ctx.clone();
                item(
                    homely(&folder.path),
                    Some(
                        if folder.tracks == 0 && scanning {
                            "Reading…".to_owned()
                        } else {
                            format!("{} tracks", crate::settings::model::count(folder.tracks))
                        }
                        .into(),
                    ),
                    vec![
                        Button::icon_only(
                            ("setup-remove-folder", index),
                            Icon::Remove,
                            label("Stop scanning this folder"),
                        )
                        .on_click(move |_, window, cx| {
                            crate::settings::panes::library::ask_remove(
                                &ctx,
                                path.clone(),
                                tracks,
                                window,
                                cx,
                            )
                        })
                        .into_any_element(),
                    ],
                    look,
                )
            })
            .collect();
        let none = rows.is_empty();
        rows.push(buttons(
            vec![
                Button::new("setup-add-folder", ButtonKind::Bordered)
                    .label(if none {
                        "Add Folder…"
                    } else {
                        "Add Another…"
                    })
                    .icon(Icon::Add)
                    .disabled(scanning)
                    .on_click(move |_, _, cx| {
                        entity.update(cx, |this, cx| {
                            let model = this.ctx.model.clone();
                            this.choosing = Some(choose_folders(model, cx));
                        })
                    })
                    .into_any_element(),
            ],
            look,
        ));
        vec![
            section(
                Some("Folders"),
                rows,
                Some(
                    if scanning {
                        "Reading your files. You can carry on meanwhile; the library fills in as they are read."
                    } else {
                        "Files dropped on the main window play where they lie, without adding their folder."
                    }
                    .into(),
                ),
                look,
            ),
        ]
    }

    fn share_question(&self, look: &Look, cx: &mut Context<Self>) -> AnyElement {
        let entity = cx.entity();
        let chosen = self.share.map(|share| share as usize);
        section(
            Some("Share this computer's library?"),
            vec![
                ChoiceCards::new("setup-share", chosen)
                    .named("share this computer's library")
                    .card(
                        Icon::Laptop,
                        "Just this computer",
                        Some("kōan plays here, and your other devices can still control it on your network."),
                    )
                    .card(
                        Icon::Phone,
                        "Share it with my devices",
                        Some("Your phone and other devices play this library, as a kōan server, while kōan is open here and this computer is awake."),
                    )
                    .on_change(move |index, window, cx| {
                        let share = *index == 1;
                        entity.update(cx, |this, cx| this.choose_sharing(share, window, cx))
                    })
                    .into_any_element(),
            ],
            None,
            look,
        )
    }

    fn away(&self, look: &Look, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let sharing = self.ctx.sharing.read(cx);
        let status = sharing.status.as_ref();
        let tailscale = status.and_then(|s| s.tailscale.clone());
        let installed = self.tailscale_installed;
        let mac_app = self.tailscale_app && cx.platform_info().os == platform::OperatingSystem::Mac;
        let busy = sharing.busy();
        let invite_to = status.map(|s| s.to);
        let serving = sharing.serving.is_some();

        let installed_item = match installed {
            None => CheckItem::new(
                "setup-ts-installed",
                Check::Looking,
                "Tailscale is installed",
            ),
            Some(true) => {
                CheckItem::new("setup-ts-installed", Check::Done, "Tailscale is installed")
            }
            Some(false) => {
                CheckItem::new("setup-ts-installed", Check::ToDo, "Tailscale is installed")
                    .detail(
                        "Install it on this computer and on your phone, and sign both in to the same account.",
                    )
                    .action(
                        Button::new("setup-ts-get", ButtonKind::Bordered)
                            .label("Get Tailscale")
                            .icon(Icon::OpenExternal)
                            .on_click(|_, _, cx| cx.open_url(TAILSCALE_DOWNLOAD)),
                    )
            }
        };
        let running_item = match (&tailscale, status, installed) {
            (Some(t), _, _) => CheckItem::new(
                "setup-ts-running",
                Check::Done,
                "Tailscale is running and signed in",
            )
            .detail(format!("This computer is {} on your tailnet.", t.name)),
            (None, None, _) | (None, _, None) => CheckItem::new(
                "setup-ts-running",
                Check::Looking,
                "Tailscale is running and signed in",
            ),
            (None, Some(_), Some(installed)) => {
                let item = CheckItem::new(
                    "setup-ts-running",
                    Check::ToDo,
                    "Tailscale is running and signed in",
                )
                .detail(match (mac_app, installed) {
                    (_, false) => "Once it is installed, sign in to it.",
                    (true, true) => "Open Tailscale and sign in. kōan looks again when you come back to this window.",
                    (false, true) => "Run \"sudo tailscale up\" in a terminal and sign in. kōan looks again when you come back to this window.",
                });
                if installed && mac_app {
                    item.action(
                        Button::new("setup-ts-open", ButtonKind::Bordered)
                            .label("Open Tailscale")
                            .on_click(|_, _, cx| cx.open_url(&format!("file://{TAILSCALE_APP}"))),
                    )
                } else {
                    item
                }
            }
        };
        let through = matches!(invite_to, Some(InviteTo::Tailnet | InviteTo::Funnel));
        let invites_item = if through {
            CheckItem::new(
                "setup-ts-invites",
                Check::Done,
                "Invites send phones through Tailscale",
            )
            .detail(
                "A phone you invite reaches this computer at its tailnet address, wherever it is.",
            )
        } else {
            let ctx = self.ctx.clone();
            CheckItem::new(
                "setup-ts-invites",
                if invite_to.is_none() {
                    Check::Looking
                } else {
                    Check::ToDo
                },
                "Invites send phones through Tailscale",
            )
            .detail("Until then, an invite carries this computer's address on your home network.")
            .action(
                Button::new("setup-ts-use", ButtonKind::Bordered)
                    .label("Use Tailscale")
                    .disabled(tailscale.is_none() || busy)
                    .on_click(move |_, _, cx| {
                        let model = ctx.model.clone();
                        let reload: Landed =
                            Box::new(move |_, cx| model.update(cx, |m, cx| m.reload(cx)));
                        ctx.sharing.update(cx, |sharing, cx| {
                            sharing.set_invite_to(InviteTo::Tailnet, None, Some(reload), cx)
                        });
                    }),
            )
        };
        let entity = cx.entity();
        let mut rows = vec![
            installed_item.into_any_element(),
            running_item.into_any_element(),
            invites_item.into_any_element(),
            buttons(
                vec![
                    Button::new("setup-ts-again", ButtonKind::Compact)
                        .label("Check Again")
                        .icon(Icon::Rescan)
                        .disabled(busy)
                        .on_click(move |_, _, cx| {
                            entity.update(cx, |this, cx| this.read_tailscale(cx))
                        })
                        .into_any_element(),
                ],
                look,
            ),
        ];
        if !serving {
            rows.push(crate::settings::form::note(
                "Sharing is off, so there is nothing for a phone to reach yet. Go back to turn it on.",
                Icon::Info,
                Tone::Muted,
                look,
            ));
        }
        vec![section(
            Some("Tailscale"),
            rows,
            Some("For phones without Tailscale, Settings → Server can publish this computer on the internet through Tailscale Funnel instead.".into()),
            look,
        )]
    }

    fn keys(&self, look: &Look, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let os = cx.platform_info().os;
        let rows = keys_worth_knowing()
            .iter()
            .enumerate()
            .map(|(row, (what, keys))| {
                Row::new(*what)
                    .control(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(look.space(Space::Xs))
                            .children(keys.iter().enumerate().map(|(k, key)| {
                                Badge::new(caption(key, os))
                                    .key((SharedString::from(format!("setup-key-{row}")), k))
                            })),
                    )
                    .into_any_element()
            })
            .collect();
        vec![
            section(Some("Keys worth knowing"), rows, None, look),
            section(
                Some("Your other devices"),
                vec![
                    text(
                        "kōan's apps for iPhone, iPad, Apple TV and Android play from this computer or your server, and can control what plays here.",
                        TypeRole::Body,
                        Tone::Ink,
                        look,
                    )
                    .into_any_element(),
                ],
                None,
                look,
            ),
        ]
    }

    /// The strip along the top, as Settings has it: it moves the window,
    /// and holds a close button where the app draws its decorations.
    fn top_strip(&self, window: &mut Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let client = draws_decorations(window);
        let look = Look::of(window, cx);
        let strip = div()
            .id("setup-top")
            .flex()
            .justify_end()
            .items_center()
            .h(crate::components::SecondaryWindow::strip_height())
            .px(look.space(Space::S));
        let entity = cx.entity();
        moves_window(strip, window, cx).when(client, |strip| {
            strip.child(close_button(move |window, cx| {
                entity.update(cx, |this, cx| this.finish(window, cx))
            }))
        })
    }

    /// Whether going on is what the step is for: not while its own action,
    /// which is then the screen's one prominent control, is still to do.
    fn done(&self, cx: &App) -> bool {
        let settings = self.ctx.model.read(cx).settings();
        match self.step {
            Step::Folders => settings.is_some_and(|s| !s.library_folders.is_empty()),
            Step::Server => settings.is_some_and(|s| s.remote_signed_in),
            Step::Share if self.share == Some(true) => self.ctx.sharing.read(cx).account.is_some(),
            _ => self.ready(),
        }
    }

    fn footer(&self, look: &Look, cx: &mut Context<Self>) -> Div {
        let steps = self.steps();
        let at = steps.iter().position(|s| *s == self.step).unwrap_or(0);
        let last = at + 1 == steps.len();
        let entity = cx.entity();
        let mut trailing = Vec::new();
        if at > 0 {
            let entity = entity.clone();
            trailing.push(
                Button::new("setup-back", ButtonKind::Standard)
                    .label("Back")
                    .on_click(move |_, window, cx| {
                        entity.update(cx, |this, cx| this.back(window, cx))
                    })
                    .into_any_element(),
            );
        }
        {
            let entity = entity.clone();
            trailing.push(
                Button::new(
                    "setup-next",
                    if self.done(cx) {
                        ButtonKind::Prominent
                    } else {
                        ButtonKind::Standard
                    },
                )
                .label(if last { "Start Listening" } else { "Continue" })
                .disabled(!self.ready())
                .on_click(move |_, window, cx| entity.update(cx, |this, cx| this.next(window, cx)))
                .into_any_element(),
            );
        }
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap(look.space(Space::M))
            .px(look.space(Space::Xl))
            .py(look.space(Space::L))
            .border_t(look.hairline)
            .border_color(look.palette.rule.hsla())
            .child(div().children((!last).then(|| {
                Button::new("setup-skip", ButtonKind::Link)
                    .label("Skip Setup")
                    .on_click(move |_, window, cx| {
                        entity.update(cx, |this, cx| this.finish(window, cx))
                    })
            })))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.space(Space::L))
                    .children(trailing),
            )
    }
}

impl Render for SetupWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let steps = self.steps();
        let at = steps.iter().position(|s| *s == self.step).unwrap_or(0);
        let header = div()
            .flex()
            .flex_col()
            .gap(look.space(Space::M))
            .px(look.space(Space::Xl))
            .pb(look.space(Space::S))
            .child(Steps::new(
                "setup-steps",
                steps.iter().map(|s| s.name()),
                at,
            ))
            .child(
                div()
                    .pt(look.space(Space::M))
                    .child(crate::components::title_text(
                        label(self.step.title()),
                        &look,
                    )),
            )
            .child(text(self.step.intro(), TypeRole::Body, Tone::Muted, &look));
        let body = self.body(&look, cx);
        // Only while there is something to say: the guide has no room for
        // an empty line.
        let model = self.ctx.model.read(cx);
        let status = (model.last_error.is_some()
            || model.last_result.is_some()
            || model.running().next().is_some())
        .then(|| crate::settings::status_line(model));
        let sheets = self.ctx.sheets.clone();
        let sheet = Sheets::render(&sheets, &look, window, cx);
        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(look.palette.bg.hsla())
            .child(self.top_strip(window, cx))
            .child(header)
            .child(div().flex_1().min_h_0().child(body))
            .children(status)
            .child(self.footer(&look, cx))
            .children(sheet)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_steps_follow_the_choices() {
        use Step::*;
        assert_eq!(steps(None, None), [Welcome, Folders, Share, Playback, Keys]);
        assert_eq!(
            steps(Some(Music::Here), Some(true)),
            [Welcome, Folders, Share, Away, Playback, Keys]
        );
        assert_eq!(
            steps(Some(Music::Here), Some(false)),
            [Welcome, Folders, Share, Playback, Keys]
        );
        assert_eq!(
            steps(Some(Music::Server), Some(true)),
            [Welcome, Server, Playback, Keys]
        );
    }

    #[test]
    fn every_key_worth_knowing_is_bound() {
        for (what, keys) in keys_worth_knowing() {
            assert!(!keys.is_empty(), "{what} has no key");
        }
    }
}
