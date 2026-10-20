//! What the page's rows do: config writes here, and everything that waits
//! on the disk or the server on a thread of its own, through the koan-core
//! calls the apps' Settings makes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use koan_core::config::Config;
use koan_core::helpers;
use koan_core::invite::Invite;
use koan_core::remote::client::{KoanInvite, SubsonicClient};
use koan_core::remote::profile;

use super::{Dialog, Done, Field, Load, Local, Op, Outcome, Remote, SettingsPage, Task};

/// A list the server offers, read; waiting forever where it does not.
fn read<T, E: std::fmt::Display>(offered: bool, list: impl FnOnce() -> Result<T, E>) -> Load<T> {
    if !offered {
        return Load::Waiting;
    }
    match list() {
        Ok(v) => Load::Ready(v),
        Err(e) => Load::Failed(e.to_string()),
    }
}

fn done(apply: impl FnOnce(&mut SettingsPage) -> Option<String> + Send + 'static) -> Done {
    Box::new(apply)
}

/// The signed-in server's client, when it offers `extension`.
fn client_offering(extension: &str) -> Result<Arc<SubsonicClient>, String> {
    let client = helpers::subsonic_client(&Config::load().unwrap_or_default())
        .ok_or("no server is signed in")?;
    if !profile::for_auth(client.auth()).is_some_and(|p| p.offers(extension)) {
        return Err("this server is older than kōan: update it to do this here".into());
    }
    Ok(client)
}

/// Adds the folder typed to the library's folders; its path as written.
pub fn add_folder(typed: &str) -> Result<String, String> {
    let typed = typed.trim();
    if typed.is_empty() {
        return Err("type the path of a folder".into());
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let path = match (typed, typed.strip_prefix("~/"), home) {
        ("~", _, Some(home)) => home,
        (_, Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(typed),
    };
    if !path.is_dir() {
        return Err(format!("{} is not a folder", path.display()));
    }
    if Config::cached().library.folders.contains(&path) {
        return Err("that folder is already in the library".into());
    }
    let shown = path.display().to_string();
    Config::persist(|c| c.library.folders.push(path))
        .map(|_| shown)
        .map_err(|e| e.to_string())
}

/// Takes `path` out of the library's folders.
pub fn remove_folder(path: &Path) -> Result<(), String> {
    if !Config::cached().library.folders.iter().any(|f| f == path) {
        return Err("that folder is no longer in the library".into());
    }
    Config::persist(|c| c.library.folders.retain(|f| f != path)).map_err(|e| e.to_string())
}

/// The dialog an invite is shown in: the link, and the password for other
/// apps when one was made.
fn invite_dialog(invite: &Invite) -> Dialog {
    let mut message = format!(
        "Opening the link on a phone, tablet or Mac with kōan installed signs in and loads \
         the library, on each device, for a week. {}",
        invite.link()
    );
    if let Some(password) = &invite.password {
        message.push_str(&format!(
            " For other Subsonic apps: server {}, username {}, password {}. Shown this once: \
             the server keeps only its hash.",
            invite.server, invite.username, password
        ));
    }
    Dialog::new(format!("invite for {}", invite.username), message)
        .choice("copy link", Op::Outcome(Outcome::Copy(invite.link())))
        .choice(
            "copy email",
            Op::Outcome(Outcome::Copy(invite.email_text())),
        )
        .choice(
            "open in mail",
            Op::Outcome(Outcome::OpenUrl(invite.mailto())),
        )
        .choice("done", Op::Cancel)
}

/// The dialog a key or app password just made is shown in, the only time.
fn secret_dialog(kind: &str, name: &str, secret: String) -> Dialog {
    Dialog::new(
        format!("{kind} for {name}"),
        format!(
            "{secret} This is the only time it is shown. Copy it into the app now; if it is \
             lost, revoke it and make another."
        ),
    )
    .choice(format!("copy {kind}"), Op::Outcome(Outcome::Copy(secret)))
    .choice("done", Op::Cancel)
}

fn account_invite(client: &SubsonicClient, made: KoanInvite) -> Invite {
    Invite::with_token(
        client.base_url(),
        &made.username,
        &made.token,
        made.password.as_deref(),
    )
}

impl SettingsPage {
    /// Reads what the page shows of this machine.
    pub(super) fn load_local(&mut self) {
        let cfg = self.cfg.clone();
        if self.editing != Some(Field::CacheLimit) {
            let limit = cfg.remote.cache_limit.clone().unwrap_or_default();
            self.fields.insert(Field::CacheLimit, limit);
        }
        self.spawn(move || {
            let mut local = Local::default();
            if let Ok(db) = koan_core::db::pool::shared().get() {
                local.folder_tracks = cfg
                    .library
                    .folders
                    .iter()
                    .map(|f| (f.clone(), helpers::tracks_under(&db, f)))
                    .collect::<HashMap<_, _>>();
                local.server_tracks = helpers::tracks_from_server(&db);
            }
            local.cache_bytes = helpers::measure_cache(&cfg);
            done(move |page| {
                page.local = local;
                None
            })
        });
    }

    /// Asks the signed-in server what it is, and for the lists it offers.
    pub(super) fn load_remote(&mut self) {
        let cfg = self.cfg.clone();
        let Some(auth) = helpers::subsonic_auth(&cfg) else {
            self.remote = Remote::default();
            return;
        };
        let url = cfg.remote.url.clone();
        if self.remote.url != url {
            self.remote = Remote {
                url: url.clone(),
                ..Default::default()
            };
        }
        self.spawn(move || {
            let mut remote = Remote {
                url: url.clone(),
                refused: helpers::sign_in_refused(&cfg),
                profile: profile::for_auth(&auth),
                ..Default::default()
            };
            let client = helpers::subsonic_client(&cfg);
            if let (Some(client), Some(p)) = (client, remote.profile.clone()) {
                remote.api_keys = read(p.offers(profile::API_KEYS), || client.koan_api_keys());
                remote.app_passwords = read(p.offers(profile::APP_PASSWORDS), || {
                    client.koan_app_passwords()
                });
                remote.scrobbling = read(p.offers(profile::SCROBBLING), || {
                    client.koan_scrobbling().map(|s| s.service)
                });
                remote.assistants = read(p.offers(profile::MCP), || client.koan_mcp());
                remote.people = client.koan_users().ok();
            }
            done(move |page| {
                if page.cfg.remote.url == url {
                    page.remote = remote;
                }
                None
            })
        });
    }

    /// Runs `work` against the server and reads the lists again, saying
    /// how it went.
    fn ask_server(
        &mut self,
        failed: &'static str,
        work: impl FnOnce() -> Result<String, String> + Send + 'static,
    ) {
        self.spawn(move || {
            let said = work();
            done(move |page| {
                page.load_remote();
                Some(said.unwrap_or_else(|e| format!("{failed}: {e}")))
            })
        });
    }

    /// Does what `op` names. Returns what the app is to do or say.
    pub(super) fn run(&mut self, op: Op) -> Option<Outcome> {
        let say = |s: &str| Some(Outcome::Say(s.to_string()));
        match op {
            Op::Outcome(o) => Some(o),
            Op::Cancel => None,
            Op::Menu(title, actions) => {
                let mut dialog = Dialog::new(title, "");
                for (label, op) in actions {
                    dialog = dialog.choice(label, op);
                }
                self.dialog = Some(dialog.choice("cancel", Op::Cancel));
                None
            }

            Op::SignIn => {
                let typed = self.field(Field::Url).trim().to_string();
                if let Some(invite) = Invite::parse(&typed) {
                    self.fields.remove(&Field::Url);
                    let server = invite.server.clone();
                    self.spawn(move || {
                        let joined = helpers::join_with_invite(&invite);
                        done(move |page| {
                            page.reload();
                            Some(match joined {
                                Ok(()) => format!("signed in to {server}"),
                                Err(e) => format!("could not join: {e}"),
                            })
                        })
                    });
                    return say("joining with the invite\u{2026}");
                }
                let (url, username) = (typed, self.field(Field::Username).trim().to_string());
                let secret = self.field(Field::Secret).to_string();
                if url.is_empty() || username.is_empty() || secret.is_empty() {
                    return say("type the server's address, a username and a password");
                }
                let with_key = self.sign_in_with_key;
                self.spawn(move || {
                    let signed = if with_key {
                        helpers::set_remote_api_key(&url, &username, &secret)
                    } else {
                        helpers::set_remote_credentials(&url, &username, &secret)
                    };
                    done(move |page| {
                        page.fields.remove(&Field::Secret);
                        page.reload();
                        Some(match signed {
                            Ok(()) => format!("signed in to {url}"),
                            Err(e) => format!("could not sign in: {e}"),
                        })
                    })
                });
                say("signing in\u{2026}")
            }
            Op::AskSignOut => {
                self.dialog = Some(
                    Dialog::new(
                        format!("sign out of {}?", self.cfg.remote.url),
                        "Tracks you also have as local files are kept either way. Keeping the \
                         rest leaves albums in the library that cannot be played until you sign \
                         in again.",
                    )
                    .choice(
                        "sign out and forget its tracks",
                        Op::SignOut { forget: true },
                    )
                    .choice("sign out and keep them", Op::SignOut { forget: false })
                    .choice("cancel", Op::Cancel),
                );
                None
            }
            Op::SignOut { forget } => {
                let out = helpers::sign_out();
                self.reload();
                match out {
                    Err(e) => Some(Outcome::Say(format!("could not sign out: {e}"))),
                    Ok(()) => Some(Outcome::SignedOut { forget }),
                }
            }

            Op::AddFolder => match add_folder(self.field(Field::Folder)) {
                Ok(_) => {
                    self.fields.remove(&Field::Folder);
                    self.reload();
                    Some(Outcome::Run(Task::Scan { force: false }))
                }
                Err(e) => {
                    self.editing = Some(Field::Folder);
                    Some(Outcome::Say(e))
                }
            },
            Op::AskRemoveFolder(path) => {
                self.dialog = Some(
                    Dialog::new(
                        format!("stop scanning {}?", path.display()),
                        "Your files are not touched either way. Keeping the tracks leaves albums \
                         in the library that kōan will not scan again.",
                    )
                    .choice(
                        "remove and forget its tracks",
                        Op::RemoveFolder {
                            path: path.clone(),
                            forget: true,
                        },
                    )
                    .choice(
                        "remove, keep them",
                        Op::RemoveFolder {
                            path,
                            forget: false,
                        },
                    )
                    .choice("cancel", Op::Cancel),
                );
                None
            }
            Op::RemoveFolder { path, forget } => {
                let removed = remove_folder(&path);
                self.reload();
                match removed {
                    Err(e) => Some(Outcome::Say(e)),
                    Ok(()) if forget => Some(Outcome::Run(Task::ForgetFolder(path))),
                    Ok(()) => Some(Outcome::Say(format!("stopped scanning {}", path.display()))),
                }
            }
            Op::AskRebuild => {
                self.dialog = Some(
                    Dialog::new(
                        "clear the library index?",
                        "Play counts, lyrics and audio analysis are lost. Favourites are kept. \
                         Your music files are not touched.",
                    )
                    .choice("clear index", Op::Outcome(Outcome::Run(Task::RebuildIndex)))
                    .choice("cancel", Op::Cancel),
                );
                None
            }
            Op::CommitCacheLimit => {
                let typed = self.field(Field::CacheLimit).trim().to_string();
                if !typed.is_empty() && koan_core::config::parse_size_bytes(&typed).is_none() {
                    self.editing = Some(Field::CacheLimit);
                    return Some(Outcome::Say(format!("{typed} is not a size, such as 50GB")));
                }
                let limit = (!typed.is_empty()).then_some(typed);
                if limit == self.cfg.remote.cache_limit {
                    return None;
                }
                let bytes = limit
                    .as_deref()
                    .and_then(koan_core::config::parse_size_bytes);
                // Asked only when the limit comes down past what is stored.
                let current = self.cfg.cache_limit_bytes();
                if bytes
                    .is_some_and(|b| b < self.local.cache_bytes && current.is_none_or(|c| b < c))
                {
                    let (title, detail) = if bytes == Some(0) {
                        (
                            "keep only what plays next?".to_string(),
                            "Tracks fetched to play that are not in the queue ahead are deleted \
                             now. Downloads you asked for and favourite records stay.",
                        )
                    } else {
                        (
                            format!(
                                "lower the limit to {}?",
                                limit.as_deref().unwrap_or_default()
                            ),
                            "Downloads over the limit are deleted now, the tracks played longest \
                             ago first. Favourite records stay.",
                        )
                    };
                    self.dialog = Some(
                        Dialog::new(title, detail)
                            .choice("delete downloads", Op::SetCacheLimit(limit))
                            .choice("cancel", Op::Cancel),
                    );
                    return None;
                }
                self.set_cache_limit(limit)
            }
            Op::SetCacheLimit(limit) => self.set_cache_limit(limit),

            Op::ChangePassword => {
                let current = self.field(Field::CurrentPassword).to_string();
                let new = self.field(Field::NewPassword).to_string();
                if current.is_empty() || new.is_empty() {
                    return say("type the current password and a new one");
                }
                self.fields.remove(&Field::CurrentPassword);
                self.fields.remove(&Field::NewPassword);
                self.ask_server("could not change the password", move || {
                    client_offering(profile::PASSWORDS)?;
                    helpers::change_own_password(&current, &new)
                        .map(|()| "password changed".to_string())
                        .map_err(|e| e.to_string())
                });
                None
            }
            Op::NewApiKey | Op::NewAppPassword => {
                let app_password = op == Op::NewAppPassword;
                let field = if app_password {
                    Field::AppPasswordName
                } else {
                    Field::KeyName
                };
                let name = self.take_field(field);
                if name.is_empty() {
                    self.editing = Some(field);
                    return say("name the app it is for");
                }
                self.spawn(move || {
                    let made = if app_password {
                        client_offering(profile::APP_PASSWORDS).and_then(|c| {
                            let made = c
                                .koan_create_app_password(&name)
                                .map_err(|e| e.to_string())?;
                            Ok((made.name, made.password))
                        })
                    } else {
                        client_offering(profile::API_KEYS).and_then(|c| {
                            let made = c.koan_create_api_key(&name).map_err(|e| e.to_string())?;
                            Ok((made.name, made.key))
                        })
                    };
                    let kind = if app_password { "app password" } else { "key" };
                    done(move |page| {
                        page.load_remote();
                        match made {
                            Ok((name, Some(secret))) => {
                                page.dialog = Some(secret_dialog(kind, &name, secret));
                                None
                            }
                            Ok((_, None)) => {
                                Some(format!("the server made the {kind} but did not send it"))
                            }
                            Err(e) => Some(format!("could not make the {kind}: {e}")),
                        }
                    })
                });
                None
            }
            Op::AskRevokeKey(id, ref name) | Op::AskRevokeAppPassword(id, ref name) => {
                let name = name.clone();
                let revoke = if matches!(op, Op::AskRevokeKey(..)) {
                    Op::RevokeKey(id)
                } else {
                    Op::RevokeAppPassword(id)
                };
                self.dialog = Some(
                    Dialog::new(
                        format!("revoke \u{201c}{name}\u{201d}?"),
                        "Whatever signs in with it stops working.",
                    )
                    .choice("revoke", revoke)
                    .choice("cancel", Op::Cancel),
                );
                None
            }
            Op::RevokeKey(id) => {
                self.ask_server("could not revoke the key", move || {
                    let c = client_offering(profile::API_KEYS)?;
                    c.koan_revoke_api_key(id).map_err(|e| e.to_string())?;
                    Ok("key revoked".into())
                });
                None
            }
            Op::RevokeAppPassword(id) => {
                self.ask_server("could not revoke the app password", move || {
                    let c = client_offering(profile::APP_PASSWORDS)?;
                    c.koan_revoke_app_password(id).map_err(|e| e.to_string())?;
                    Ok("app password revoked".into())
                });
                None
            }

            Op::AddPerson | Op::Invite { .. } => {
                let (username, role, reset) = match &op {
                    Op::Invite { username, reset } => (username.clone(), None, *reset),
                    _ => (
                        self.take_field(Field::PersonName),
                        Some(self.new_role),
                        false,
                    ),
                };
                if username.is_empty() {
                    self.editing = Some(Field::PersonName);
                    return say("type their username");
                }
                self.spawn(move || {
                    let made = client_offering(profile::INVITE).and_then(|c| {
                        let made = match role {
                            Some(role) => c.koan_create_user(&username, role),
                            None => c.koan_invite(&username, reset),
                        }
                        .map_err(|e| e.to_string())?;
                        Ok(account_invite(&c, made))
                    });
                    done(move |page| {
                        page.load_remote();
                        match made {
                            Ok(invite) => {
                                page.dialog = Some(invite_dialog(&invite));
                                None
                            }
                            Err(e) => Some(format!("could not make the invite: {e}")),
                        }
                    })
                });
                None
            }
            Op::AskReset(username) => {
                self.dialog = Some(
                    Dialog::new(
                        format!("give {username} a new password?"),
                        "The invite carries the new password. Their devices will have to sign in again.",
                    )
                    .choice(
                        "new password and invite",
                        Op::Invite {
                            username,
                            reset: true,
                        },
                    )
                    .choice("cancel", Op::Cancel),
                );
                None
            }
            Op::AskSetPassword(username) => {
                let mut dialog = Dialog::new(
                    format!("set a password for {username}"),
                    "Type it, then set. Their devices will have to sign in again with it.",
                )
                .choice("set", Op::SetPassword(username))
                .choice("cancel", Op::Cancel);
                dialog.input = Some(String::new());
                self.dialog = Some(dialog);
                None
            }
            Op::SetPassword(username) => self.set_password(username, String::new()),
            Op::AskDelete(username) => {
                self.dialog = Some(
                    Dialog::new(
                        format!("delete {username}?"),
                        "Their devices stop working, and their playlists and favourites go.",
                    )
                    .choice("delete", Op::Delete(username))
                    .choice("cancel", Op::Cancel),
                );
                None
            }
            Op::Delete(username) => {
                self.ask_server("could not delete the account", move || {
                    let c = helpers::subsonic_client(&Config::load().unwrap_or_default())
                        .ok_or("no server is signed in")?;
                    c.koan_delete_user(&username).map_err(|e| e.to_string())?;
                    Ok(format!("deleted {username}"))
                });
                None
            }
            Op::AskRole(username, current) => {
                let mut dialog = Dialog::new(
                    format!("{username}'s access"),
                    format!("They can {} now.", super::role_label(&current)),
                );
                for (role, label) in super::ROLES {
                    dialog = if role == current {
                        dialog.choice(format!("{label} \u{2713}"), Op::Cancel)
                    } else {
                        dialog.choice(label, Op::SetRole(username.clone(), role.into()))
                    };
                }
                self.dialog = Some(dialog.choice("cancel", Op::Cancel));
                None
            }
            Op::SetRole(username, role) => {
                self.ask_server("could not change their access", move || {
                    let c = helpers::subsonic_client(&Config::load().unwrap_or_default())
                        .ok_or("no server is signed in")?;
                    c.koan_set_user_role(&username, &role)
                        .map_err(|e| e.to_string())?;
                    Ok(format!("{username} can now {}", super::role_label(&role)))
                });
                None
            }

            Op::Pair => {
                let code = self.take_field(Field::PairCode);
                if code.is_empty() {
                    return say("type the code the device shows");
                }
                self.spawn(move || {
                    let info = koan_core::remote::pair::info(&code);
                    done(move |page| match info {
                        Ok(p) => {
                            let from = if p.from.is_empty() {
                                String::new()
                            } else {
                                format!(" It asked from {}.", p.from)
                            };
                            page.dialog = Some(
                                Dialog::new(
                                    format!("sign in {}?", p.device),
                                    format!(
                                        "It signs in as you, with a key of its own that can be \
                                         revoked on the server.{from}"
                                    ),
                                )
                                .choice("approve", Op::ApprovePair(code.clone()))
                                .choice("decline", Op::DeclinePair(code))
                                .choice("cancel", Op::Cancel),
                            );
                            None
                        }
                        Err(e) => Some(format!("could not pair: {e}")),
                    })
                });
                None
            }
            Op::ApprovePair(ref code) | Op::DeclinePair(ref code) => {
                let code = code.clone();
                let approve = matches!(op, Op::ApprovePair(_));
                self.ask_server("could not answer the device", move || {
                    use koan_core::remote::pair;
                    if approve {
                        pair::approve(&code).map(|name| format!("signed in {name}"))
                    } else {
                        pair::decline(&code).map(|_| "declined".to_string())
                    }
                    .map_err(|e| e.to_string())
                });
                None
            }

            Op::ConnectScrobbling => {
                let token = self.take_field(Field::ScrobbleToken);
                if token.is_empty() {
                    self.editing = Some(Field::ScrobbleToken);
                    return say("paste your ListenBrainz user token");
                }
                self.ask_server("could not connect ListenBrainz", move || {
                    koan_core::remote::scrobbling::connect(&token)
                        .map(|s| match s {
                            Some(s) => format!("scrobbling to ListenBrainz as {}", s.account),
                            None => "ListenBrainz connected".into(),
                        })
                        .map_err(|e| e.to_string())
                });
                None
            }
            Op::DisconnectScrobbling => {
                self.ask_server("could not disconnect ListenBrainz", || {
                    koan_core::remote::scrobbling::disconnect()
                        .map(|()| "ListenBrainz disconnected".to_string())
                        .map_err(|e| e.to_string())
                });
                None
            }

            Op::EndConnection { key, refuse } => {
                let mut wrote = Ok(());
                if let Some(addr) = refuse
                    && !self.cfg.devices.refused.contains(&addr)
                {
                    wrote = self.persist(|c| c.devices.refused.push(addr));
                }
                koan_core::remote::nearby::end(key);
                self.follow_devices(true);
                wrote.err().map(Outcome::Say)
            }
            Op::AllowHeld(addr) => {
                koan_core::remote::nearby::release_addr(&addr);
                self.follow_devices(true);
                None
            }
            Op::AllowRefused(addr) => self
                .persist(|c| c.devices.refused.retain(|a| *a != addr))
                .err()
                .map(Outcome::Say),
            Op::AddAddress => {
                let addr = self.take_field(Field::Address);
                if addr.is_empty() || self.cfg.devices.addresses.contains(&addr) {
                    return None;
                }
                self.persist(|c| c.devices.addresses.push(addr))
                    .err()
                    .map(Outcome::Say)
            }
            Op::RemoveAddress(addr) => self
                .persist(|c| c.devices.addresses.retain(|a| *a != addr))
                .err()
                .map(Outcome::Say),
            Op::Share | Op::StopSharing(_) => {
                let (account, allow) = match op {
                    Op::StopSharing(account) => (account, false),
                    _ => (self.take_field(Field::Grantee), true),
                };
                Some(Outcome::Say(
                    match koan_core::remote::devices::share(&account, allow) {
                        Ok(()) if allow => format!("sharing this device with {account}"),
                        Ok(()) => format!("stopped sharing with {account}"),
                        Err(e) => e,
                    },
                ))
            }
        }
    }

    /// Writes the cache limit and says what it now keeps.
    fn set_cache_limit(&mut self, limit: Option<String>) -> Option<Outcome> {
        let said = match &limit {
            Some(l) if koan_core::config::parse_size_bytes(l) == Some(0) => {
                "no cache: only the queue ahead and downloads you asked for are kept".into()
            }
            Some(l) => format!("downloads kept to {l}"),
            None => "no limit on downloads".into(),
        };
        let wrote = self.persist(|c| c.remote.cache_limit = limit);
        if wrote.is_ok() {
            koan_core::remote::queue::cache_limit_changed();
        }
        Some(Outcome::Say(wrote.map_or_else(|e| e, |()| said)))
    }

    /// Gives `username` the password typed into the dialog.
    pub(super) fn set_password(&mut self, username: String, password: String) -> Option<Outcome> {
        if password.is_empty() {
            return Some(Outcome::Say("type the new password".into()));
        }
        self.ask_server("could not set the password", move || {
            let c = client_offering(profile::PASSWORDS)?;
            c.koan_set_user_password(&username, &password)
                .map_err(|e| e.to_string())?;
            Ok(format!("{username} has a new password"))
        });
        None
    }

    /// Whether the page is signed in to a server, for its tests.
    #[cfg(test)]
    pub(super) fn signed_in(&self) -> bool {
        super::signed_in(&self.cfg)
    }
}
