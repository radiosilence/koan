//! What each section of the page holds, as rows, from the page's state.
//! Rows are made again each frame: nothing here is stored.

use koan_core::remote::connections::{Connection, Via};
use koan_core::remote::profile::{
    self, API_KEYS, APP_PASSWORDS, MCP, PAIR, PASSWORDS, SCROBBLING, SHARES,
};

use super::{
    Field, Load, Op, Outcome, Section, Setting, SettingsPage, Task, role_label, signed_in,
};
use crate::text::{bytes, count};

#[derive(Debug, Clone)]
pub(crate) struct Row {
    /// Who the row is, whatever place it has.
    pub key: String,
    pub kind: Kind,
}

#[derive(Debug, Clone)]
pub(crate) enum Kind {
    Heading(&'static str),
    /// Text under a group, wrapped.
    Note(String),
    /// Something wrong, wrapped.
    Warn(String),
    /// A value shown; with `op`, what picking it does.
    Info {
        label: &'static str,
        value: String,
        op: Option<Op>,
    },
    Choice {
        label: &'static str,
        value: String,
        note: &'static str,
        step: Step,
    },
    /// A text to type, and what `Enter` does with it.
    Field {
        field: Field,
        label: &'static str,
        submit: Option<Op>,
    },
    Button {
        label: String,
        note: String,
        op: Op,
    },
    /// A thing in a list, with what can be done to it.
    Item {
        title: String,
        detail: String,
        actions: Vec<(&'static str, Op)>,
        /// What `x` does.
        remove: Option<Op>,
    },
}

impl Kind {
    pub fn selectable(&self) -> bool {
        match self {
            Kind::Heading(_) | Kind::Note(_) | Kind::Warn(_) => false,
            Kind::Info { op, .. } => op.is_some(),
            Kind::Choice { .. } | Kind::Field { .. } | Kind::Button { .. } | Kind::Item { .. } => {
                true
            }
        }
    }
}

/// What stepping a choice changes.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Step {
    Setting(Setting),
    /// A password or an API key, signing in.
    SignInWith,
    /// The access a new person is given.
    NewRole,
}

#[derive(Default)]
struct Rows(Vec<Row>);

impl Rows {
    fn push(&mut self, key: impl Into<String>, kind: Kind) {
        self.0.push(Row {
            key: key.into(),
            kind,
        });
    }

    fn heading(&mut self, name: &'static str) {
        self.push(format!("heading:{name}"), Kind::Heading(name));
    }

    fn note(&mut self, text: impl Into<String>) {
        self.push("", Kind::Note(text.into()));
    }

    fn warn(&mut self, text: impl Into<String>) {
        self.push("", Kind::Warn(text.into()));
    }

    fn info(&mut self, label: &'static str, value: impl Into<String>) {
        self.push(
            format!("info:{label}"),
            Kind::Info {
                label,
                value: value.into(),
                op: None,
            },
        );
    }

    fn link(&mut self, label: &'static str, value: impl Into<String>, op: Op) {
        self.push(
            format!("info:{label}"),
            Kind::Info {
                label,
                value: value.into(),
                op: Some(op),
            },
        );
    }

    fn setting(&mut self, page: &SettingsPage, setting: Setting) {
        self.push(
            format!("setting:{setting:?}"),
            Kind::Choice {
                label: setting.label(),
                value: setting.value(&page.cfg),
                note: setting.note(),
                step: Step::Setting(setting),
            },
        );
    }

    fn field(&mut self, field: Field, label: &'static str, submit: Option<Op>) {
        self.push(
            format!("field:{field:?}"),
            Kind::Field {
                field,
                label,
                submit,
            },
        );
    }

    fn button(&mut self, label: impl Into<String>, note: impl Into<String>, op: Op) {
        let label = label.into();
        self.push(
            format!("button:{label}"),
            Kind::Button {
                label,
                note: note.into(),
                op,
            },
        );
    }

    fn item(
        &mut self,
        key: String,
        title: impl Into<String>,
        detail: impl Into<String>,
        actions: Vec<(&'static str, Op)>,
        remove: Option<Op>,
    ) {
        self.push(
            key,
            Kind::Item {
                title: title.into(),
                detail: detail.into(),
                actions,
                remove,
            },
        );
    }
}

pub(crate) fn build(page: &SettingsPage) -> Vec<Row> {
    let mut rows = Rows::default();
    match page.section {
        Section::Library => library(page, &mut rows),
        Section::Server => server(page, &mut rows),
        Section::Account => account(page, &mut rows),
        Section::People => people(page, &mut rows),
        Section::Playback => playback(page, &mut rows),
        Section::Eq => eq(page, &mut rows),
        Section::Devices => devices(page, &mut rows),
        Section::Integrations => integrations(page, &mut rows),
        Section::Terminal => {
            rows.heading("terminal");
            for s in [
                Setting::Theme,
                Setting::CoverGraphics,
                Setting::ArtSize,
                Setting::ShowFps,
                Setting::FrameRate,
                Setting::Palette,
            ] {
                rows.setting(page, s);
            }
            rows.note(
                "How this terminal draws kōan. The apps keep their appearance to themselves.",
            );
        }
    }
    rows.0
}

/// An ISO 8601 time as a date.
fn date(iso: &str) -> Option<String> {
    let t = chrono::DateTime::parse_from_rfc3339(iso).ok()?;
    Some(
        t.with_timezone(&chrono::Local)
            .format("%-d %b %Y")
            .to_string(),
    )
}

/// When a key or password was made and last used.
fn made_and_used(created: Option<&str>, used: Option<&str>) -> String {
    let made = created.and_then(date).map(|d| format!("made {d}"));
    let used = used
        .and_then(date)
        .map_or("never used".to_string(), |d| format!("last used {d}"));
    made.into_iter()
        .chain([used])
        .collect::<Vec<_>>()
        .join(" \u{b7} ")
}

fn library(page: &SettingsPage, rows: &mut Rows) {
    rows.heading("folders");
    let folders = &page.cfg.library.folders;
    if folders.is_empty() {
        rows.note("No folders yet: kōan has nothing to scan.");
    }
    for f in folders {
        let tracks = page
            .local
            .folder_tracks
            .get(f)
            .map(|n| count(*n, "track"))
            .unwrap_or_default();
        let remove = Op::AskRemoveFolder(f.clone());
        rows.item(
            format!("folder:{}", f.display()),
            f.display().to_string(),
            tracks,
            vec![("remove", remove.clone())],
            Some(remove),
        );
    }
    rows.field(Field::Folder, "add folder", Some(Op::AddFolder));
    rows.note("Removing a folder stops it being scanned. It does not delete anything.");

    rows.heading("scan");
    rows.button(
        "scan",
        "new, changed and removed files",
        Op::Outcome(Outcome::Run(Task::Scan { force: false })),
    );
    rows.button(
        "rescan everything",
        "re-read every file's tags, ignoring the scan cache",
        Op::Outcome(Outcome::Run(Task::Scan { force: true })),
    );

    rows.heading("rebuild");
    rows.button("clear library index\u{2026}", "", Op::AskRebuild);
    rows.note(
        "Forgets every artist, album and track so the next scan builds them again from \
         your files. Favourites survive: they are kept against file paths. Lyrics, play \
         counts and audio analysis do not; they are tied to rows that will not exist.",
    );
}

fn server(page: &SettingsPage, rows: &mut Rows) {
    let cfg = &page.cfg;
    let remote = &page.remote;
    if signed_in(cfg) {
        rows.heading("signed in");
        rows.link(
            "server",
            cfg.remote.url.clone(),
            Op::Outcome(Outcome::Copy(cfg.remote.url.clone())),
        );
        rows.info("user", cfg.remote.username.clone());
        rows.info("tracks", page.local.server_tracks.to_string());
        if remote.refused {
            rows.warn("Your server refused kōan's sign-in. Sign out, then sign in again.");
        }
        rows.button(
            "sync",
            "pull the library, favourites and playlists",
            Op::Outcome(Outcome::Run(Task::Sync)),
        );
        rows.button("sign out\u{2026}", "", Op::AskSignOut);
        rows.setting(page, Setting::Offline);
        rows.note(
            "Offline shows only what is on this device. It turns on by itself when your \
             server cannot be reached, and off again when it can.",
        );
    } else {
        rows.heading("subsonic or navidrome");
        rows.field(Field::Url, "server url", Some(Op::SignIn));
        rows.field(Field::Username, "username", None);
        rows.push(
            "choice:sign in with",
            Kind::Choice {
                label: "sign in with",
                value: if page.sign_in_with_key {
                    "api key"
                } else {
                    "password"
                }
                .into(),
                note: "",
                step: Step::SignInWith,
            },
        );
        let label = if page.sign_in_with_key {
            "api key"
        } else {
            "password"
        };
        rows.field(Field::Secret, label, Some(Op::SignIn));
        rows.button("sign in", "", Op::SignIn);
        rows.note(
            "Paste an invite into server url and kōan fills in the rest. The account is \
             checked against the server, then saved to config.local.toml, readable only by you.",
        );
    }

    rows.heading("automatic sync");
    rows.setting(page, Setting::AutoSync);
    if cfg.remote.auto_sync {
        rows.setting(page, Setting::SyncInterval);
    }
    rows.note(
        "The apps sync on their own, each time asking the server only for what changed \
         since the last. The terminal syncs when sync is run.",
    );

    rows.heading("downloads");
    rows.setting(page, Setting::DownloadWorkers);
    rows.field(Field::CacheLimit, "cache limit", Some(Op::CommitCacheLimit));
    rows.info("using", bytes(page.local.cache_bytes));
    rows.button(
        "clear downloads",
        "the library keeps the tracks; they download again to play",
        Op::Outcome(Outcome::Run(Task::ClearCache)),
    );

    if signed_in(cfg) {
        rows.heading("what the server offers");
        match &remote.profile {
            Some(p) if p.kind.is_some() || p.open_subsonic => {
                let name = match p.kind.as_deref() {
                    Some("koan") => "kōan".to_string(),
                    Some(kind) => {
                        let mut c = kind.chars();
                        c.next()
                            .map(|f| f.to_uppercase().chain(c).collect())
                            .unwrap_or_default()
                    }
                    None => "Subsonic".into(),
                };
                let version = p.version.as_deref().unwrap_or("");
                rows.info("server", format!("{name} {version}").trim().to_string());
                rows.info("opensubsonic", if p.open_subsonic { "yes" } else { "no" });
                let devices = if !p.offers(profile::DEVICES) {
                    "not offered"
                } else if koan_core::remote::devices::linked() {
                    "connected"
                } else {
                    "offered, not connected"
                };
                rows.info("your devices", devices);
                if !p.extensions.is_empty() {
                    rows.info("extensions", p.extensions.len().to_string());
                    let names: Vec<String> = p
                        .extensions
                        .iter()
                        .map(|(name, versions)| {
                            let v: Vec<String> = versions.iter().map(|v| format!("v{v}")).collect();
                            format!("{name} {}", v.join(", "))
                        })
                        .collect();
                    rows.note(names.join(" \u{b7} "));
                }
            }
            _ => rows.note("Not reached yet."),
        }
        rows.note(
            "Asked when kōan signs in. Features beyond Subsonic are used only where the \
             server lists them.",
        );
    }
}

fn account(page: &SettingsPage, rows: &mut Rows) {
    let remote = &page.remote;
    if remote.offers(PASSWORDS) {
        rows.heading("password");
        rows.info("user", page.cfg.remote.username.clone());
        rows.field(Field::CurrentPassword, "current password", None);
        rows.field(Field::NewPassword, "new password", Some(Op::ChangePassword));
        rows.button("change password", "", Op::ChangePassword);
        rows.note(
            "This device stays signed in. Your other devices, and other apps using this \
             account, will have to sign in again.",
        );
    }
    if remote.offers(API_KEYS) {
        rows.heading("api keys");
        match &remote.api_keys {
            Load::Ready(keys) => {
                for k in keys {
                    let (actions, remove, detail) = if k.current {
                        (Vec::new(), None, "this device".to_string())
                    } else {
                        let revoke = Op::AskRevokeKey(k.id, k.name.clone());
                        (
                            vec![("revoke", revoke.clone())],
                            Some(revoke),
                            made_and_used(k.created.as_deref(), k.last_used.as_deref()),
                        )
                    };
                    rows.item(
                        format!("key:{}", k.id),
                        k.name.clone(),
                        detail,
                        actions,
                        remove,
                    );
                }
                rows.field(Field::KeyName, "new key for", Some(Op::NewApiKey));
                rows.button("new key", "", Op::NewApiKey);
            }
            Load::Waiting => rows.note("\u{2026}"),
            Load::Failed(e) => rows.warn(e.clone()),
        }
        rows.note(
            "A key signs another Subsonic app in as you, without your password. Each device \
             kōan is signed in on has one too; revoking it signs that device out.",
        );
    }
    if remote.offers(APP_PASSWORDS) && !koan_core::remote::offline::active() {
        rows.heading("app passwords");
        match &remote.app_passwords {
            Load::Ready(passwords) => {
                for p in passwords {
                    let revoke = Op::AskRevokeAppPassword(p.id, p.name.clone());
                    rows.item(
                        format!("app-password:{}", p.id),
                        p.name.clone(),
                        made_and_used(p.created.as_deref(), p.last_used.as_deref()),
                        vec![("revoke", revoke.clone())],
                        Some(revoke),
                    );
                }
                rows.field(
                    Field::AppPasswordName,
                    "new password for",
                    Some(Op::NewAppPassword),
                );
                rows.button("new app password", "", Op::NewAppPassword);
            }
            Load::Waiting => rows.note("\u{2026}"),
            Load::Failed(e) => rows.warn(e.clone()),
        }
        rows.note(
            "For Subsonic apps that sign in only with a token and salt, and so cannot use \
             an API key. Changing your password revokes them all.",
        );
    }
}

fn people(page: &SettingsPage, rows: &mut Rows) {
    rows.heading("people");
    let me = &page.cfg.remote.username;
    for user in page.remote.people.iter().flatten() {
        let name = &user.username;
        let next = super::cycle_role(&user.role, true);
        let mut actions = vec![
            ("access", Op::SetRole(name.clone(), next.into())),
            (
                "invite",
                Op::Invite {
                    username: name.clone(),
                    reset: false,
                },
            ),
        ];
        let mut remove = None;
        if name != me {
            if page.remote.offers(PASSWORDS) {
                actions.push(("set password", Op::AskSetPassword(name.clone())));
            }
            actions.push(("new password", Op::AskReset(name.clone())));
            actions.push(("delete", Op::AskDelete(name.clone())));
            remove = Some(Op::AskDelete(name.clone()));
        }
        let title = if name == me {
            format!("{name} (you)")
        } else {
            name.clone()
        };
        rows.item(
            format!("person:{name}"),
            title,
            role_label(&user.role),
            actions,
            remove,
        );
    }
    rows.field(Field::PersonName, "new person", Some(Op::AddPerson));
    rows.push(
        "choice:new role",
        Kind::Choice {
            label: "their access",
            value: role_label(page.new_role).into(),
            note: "",
            step: Step::NewRole,
        },
    );
    rows.button("add", "", Op::AddPerson);
    rows.note("Adding someone makes their invite: one link that sets kōan up with the account.");
}

fn playback(page: &SettingsPage, rows: &mut Rows) {
    rows.heading("device");
    let output = page
        .cfg
        .playback
        .output_device
        .clone()
        .unwrap_or_else(|| "system default".into());
    rows.link("output", output, Op::Outcome(Outcome::OpenOutputs));
    rows.note(
        "kōan asks the device to run at the source's sample rate, so nothing is resampled \
         unless the device refuses.",
    );
    rows.heading("transport");
    rows.setting(page, Setting::FadeOnPause);
    rows.heading("loudness");
    rows.setting(page, Setting::ReplayGain);
    if page.cfg.playback.replaygain != koan_core::config::ReplayGainMode::Off {
        rows.setting(page, Setting::PreAmp);
    }
    rows.note(
        "Applies the gain written into the file's tags. Per album keeps the relative \
         loudness within an album.",
    );
}

fn eq(page: &SettingsPage, rows: &mut Rows) {
    rows.heading("eq");
    rows.link(
        "in use",
        page.eq_summary
            .clone()
            .unwrap_or_else(|| "see the eq page".into()),
        Op::Outcome(Outcome::OpenEq),
    );
    rows.button("open the eq page", "", Op::Outcome(Outcome::OpenEq));
    rows.note("Profiles are chosen, edited and imported on the EQ page; E opens it from anywhere.");
}

/// What a connection is, and since when.
fn connection_detail(c: &Connection) -> String {
    use koan_core::remote::proof::Peer;
    let who = match &c.peer {
        Some(Peer::Own) => "your device".to_string(),
        Some(Peer::Shared(owner)) => format!("shared by {owner}"),
        None => "not proved".into(),
    };
    let what = match (c.via, c.inbound) {
        (Via::Server, false) => "your server".to_string(),
        (Via::Server, true) => "your device, through the server".into(),
        (Via::Network, true) => format!("can control this device on this network, {who}"),
        (Via::Network, false) => format!("this device is connected to it on this network, {who}"),
    };
    match c.since.and_then(|t| chrono::DateTime::from_timestamp(t, 0)) {
        Some(at) => {
            let at = at.with_timezone(&chrono::Local);
            let today = chrono::Local::now().date_naive() == at.date_naive();
            let when = if today {
                at.format("%H:%M")
            } else {
                at.format("%-d %b %H:%M")
            };
            format!("{what} \u{b7} since {when}")
        }
        None => what,
    }
}

fn devices(page: &SettingsPage, rows: &mut Rows) {
    let cfg = &page.cfg;
    if signed_in(cfg) && page.remote.offers(PAIR) {
        rows.heading("pair a device");
        rows.field(Field::PairCode, "code", Some(Op::Pair));
        rows.note(
            "A television or another device without a keyboard shows a code while it waits. \
             Enter it here to sign it in as you, with a key of its own that can be revoked \
             on the server.",
        );
    }

    rows.heading("this device");
    rows.setting(page, Setting::Discoverable);
    rows.setting(page, Setting::NearbyControl);
    if let Some(port) = koan_core::remote::nearby::listening_port() {
        rows.info("listening on port", port.to_string());
    }
    rows.note(
        "Other kōan apps on this network can see and control what plays here. Full control \
         adds the output, preset, volume and moving the music; playback only is play and the \
         queue. Neither reaches your library. On a network shared with strangers, choose \
         playback only.",
    );

    let refused = &cfg.devices.refused;
    if !page.connections.is_empty() || !page.held.is_empty() || !refused.is_empty() {
        rows.heading("connected now");
        for (i, c) in page.connections.iter().enumerate() {
            let mut actions = Vec::new();
            let mut remove = None;
            if let Some(key) = c.key {
                if c.peer.is_none()
                    && let Some(addr) = &c.addr
                {
                    actions.push((
                        "refuse",
                        Op::EndConnection {
                            key,
                            refuse: Some(addr.clone()),
                        },
                    ));
                }
                let end = Op::EndConnection { key, refuse: None };
                actions.push(("disconnect", end.clone()));
                remove = Some(end);
            }
            // By place as well: two of the account's devices can share a name.
            let key = format!("connection:{i}:{}:{:?}", c.name, c.id);
            rows.item(key, c.name.clone(), connection_detail(c), actions, remove);
        }
        for h in &page.held {
            let allow = Op::AllowHeld(h.addr.clone());
            rows.item(
                format!("held:{}", h.addr),
                h.name.clone().unwrap_or_else(|| h.addr.clone()),
                "disconnected until allowed back",
                vec![("allow", allow)],
                None,
            );
        }
        for addr in refused {
            rows.item(
                format!("refused:{addr}"),
                addr.clone(),
                "refused",
                vec![("allow", Op::AllowRefused(addr.clone()))],
                None,
            );
        }
        rows.note(
            "Disconnect hangs up a device on this network and keeps it from connecting again \
             until you allow it back, play on it, or kōan restarts. Refuse keeps a device \
             that has not proved it is yours out for good, by its address. Your account's \
             devices reach this one through the server while they are signed in.",
        );
    }

    rows.heading("devices by address");
    for addr in &cfg.devices.addresses {
        let remove = Op::RemoveAddress(addr.clone());
        rows.item(
            format!("address:{addr}"),
            addr.clone(),
            "",
            vec![("remove", remove.clone())],
            Some(remove),
        );
    }
    rows.field(Field::Address, "add address", Some(Op::AddAddress));
    rows.note(
        "For networks that do not announce devices, such as a tailnet. The port is 5626 \
         unless the other device says otherwise.",
    );

    if signed_in(cfg) && page.remote.offers(SHARES) {
        rows.heading("shared with other accounts");
        for account in koan_core::remote::devices::shares() {
            let stop = Op::StopSharing(account.clone());
            rows.item(
                format!("share:{account}"),
                account,
                "",
                vec![("stop sharing", stop.clone())],
                Some(stop),
            );
        }
        rows.field(Field::Grantee, "share with", Some(Op::Share));
        if let Some(e) = koan_core::remote::devices::share_error() {
            rows.warn(e);
        }
        let shared = koan_core::remote::devices::shares();
        let others: Vec<String> = koan_core::remote::devices::accounts()
            .into_iter()
            .filter(|a| !shared.contains(a))
            .collect();
        if !others.is_empty() {
            rows.note(format!("On this server: {}", others.join(", ")));
        }
        rows.note(
            "From any network, as their own account, they can see and control what this device \
             plays, including the output, preset and volume. Nothing of your library, \
             playlists, history or settings.",
        );
    }
}

fn integrations(page: &SettingsPage, rows: &mut Rows) {
    let remote = &page.remote;
    if remote.offers(MCP) {
        rows.heading("assistants");
        match &remote.assistants {
            Load::Ready(mcp) => {
                rows.link(
                    "address",
                    mcp.url.clone(),
                    Op::Outcome(Outcome::Copy(mcp.url.clone())),
                );
                rows.button(
                    "how to connect an assistant",
                    "",
                    Op::Outcome(Outcome::OpenUrl(mcp.connect.clone())),
                );
            }
            Load::Waiting => rows.note("\u{2026}"),
            Load::Failed(e) => rows.warn(e.clone()),
        }
        rows.note(
            "Add the address to Claude or another assistant that speaks MCP as a custom \
             connector. It signs in through your server, and can then search your library, \
             make playlists and play music on your devices, as you.",
        );
    }
    if remote.offers(SCROBBLING) {
        rows.heading("scrobbling");
        match &remote.scrobbling {
            Load::Ready(services) => {
                let find = |name: &str| services.iter().find(|s| s.name == name);
                match find("listenbrainz") {
                    Some(s) => {
                        rows.info("listenbrainz", format!("as {}", s.account));
                        if let Some(e) = &s.error {
                            rows.warn(e.clone());
                            rows.note(
                                "Disconnect, then connect again with a current token. Plays \
                                 recorded meanwhile are kept and sent.",
                            );
                        } else if s.pending > 0 {
                            rows.note(format!(
                                "{} waiting to be sent",
                                count(s.pending as u64, "play")
                            ));
                        }
                        rows.button("disconnect listenbrainz", "", Op::DisconnectScrobbling);
                    }
                    None => {
                        rows.field(
                            Field::ScrobbleToken,
                            "listenbrainz token",
                            Some(Op::ConnectScrobbling),
                        );
                        rows.button("connect listenbrainz", "", Op::ConnectScrobbling);
                        rows.button(
                            "find your token",
                            "listenbrainz.org/settings",
                            Op::Outcome(Outcome::OpenUrl(
                                "https://listenbrainz.org/settings/".into(),
                            )),
                        );
                    }
                }
                let page_url = format!("{}/scrobbling", page.cfg.remote.url.trim_end_matches('/'));
                match find("lastfm") {
                    Some(s) => {
                        rows.info("last.fm", format!("as {}", s.account));
                        if let Some(e) = &s.error {
                            rows.warn(e.clone());
                        }
                        rows.button(
                            "manage last.fm on the server",
                            "",
                            Op::Outcome(Outcome::OpenUrl(page_url)),
                        );
                    }
                    None => rows.button(
                        "connect last.fm on the server",
                        "Last.fm signs in through the server's own page",
                        Op::Outcome(Outcome::OpenUrl(page_url)),
                    ),
                }
            }
            Load::Waiting => rows.note("Checking\u{2026}"),
            Load::Failed(e) => rows.warn(e.clone()),
        }
        rows.note(
            "The server sends what you play, from every app signed in as you, your history \
             included when you connect. The token is kept on the server.",
        );
    }
}
