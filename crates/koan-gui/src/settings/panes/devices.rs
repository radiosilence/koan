//! Devices: being found and controlled by kōan apps on the same network,
//! reaching devices on networks that do not announce them, who is
//! connected now, and the accounts this device is shared with. With a
//! server: approving a device that signs in by code, and keeping the queue
//! on the server.

use crate::components::{Button, ButtonKind, Picker, TextField, Toggle};
use crate::theme::{Icon, Look, Tone, TypeRole};
use koan_ffi::{ConnectedInfo, ServerQueue};
use koan_kit::app::input::{InputEvent, InputState};
use koan_kit::app::*;
// sizes and colours computed at runtime from measurements, not chosen from the theme.
use koan_kit::raw::px;

use super::server::host;
use crate::settings::Ctx;
use crate::settings::form::{Row, buttons, item, moment, note, page, section, text};
use crate::settings::model::describe;
use crate::settings::sheet::{Action, Sheet};

const NEARBY: [(&str, &str); 2] = [("Full control", "full"), ("Playback only", "playback")];

pub struct DevicesPane {
    ctx: Ctx,
    code: Entity<InputState>,
    address: Entity<InputState>,
    grantee: Entity<InputState>,
    share_error: Option<String>,
    /// The lookup a click starts, one of each; a new click replaces it.
    pairing: Option<Task<()>>,
    queue_lookup: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl DevicesPane {
    pub fn new(ctx: Ctx, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let field = |placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        let code = field("XXXX-XXXX", window, cx);
        let address = field("host or host:port", window, cx);
        let grantee = field("Their username on this server", window, cx);
        let subscriptions = vec![
            cx.observe(&ctx.model, |_, _, cx| cx.notify()),
            cx.observe(&ctx.connection, |_, _, cx| cx.notify()),
            cx.observe(&ctx.sharing, |_, _, cx| cx.notify()),
            cx.observe(&address, |_, _, cx| cx.notify()),
            cx.observe(&grantee, |_, _, cx| cx.notify()),
            // The code reads as the device shows it, however it is typed or
            // pasted.
            cx.subscribe_in(
                &code,
                window,
                |pane, state, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => {
                        let typed = state.read(cx).value().to_string();
                        let shown = koan_ffi::format_pair_code(typed.clone());
                        if shown != typed {
                            state.update(cx, |s, cx| s.set_value(shown, window, cx));
                        }
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } => pane.approve(window, cx),
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &address,
                window,
                |pane, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        pane.add_address(window, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &grantee,
                window,
                |pane, state, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        let name = state.read(cx).value().trim().to_owned();
                        pane.grant(name, window, cx);
                    }
                },
            ),
        ];
        Self {
            ctx,
            code,
            address,
            grantee,
            share_error: None,
            pairing: None,
            queue_lookup: None,
            _subscriptions: subscriptions,
        }
    }

    /// Asks the server which device is waiting on the code, then the person:
    /// approving signs it in as this account, with a key of its own.
    fn approve(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let typed = self.code.read(cx).value().trim().to_owned();
        if typed.is_empty() {
            return;
        }
        self.code.update(cx, |s, cx| s.set_value("", window, cx));
        self.approve_pair(typed, window, cx);
    }

    /// The same question for `pair`, a pairing's id or a typed code: from
    /// the field, or from a `koan.rocks/pair/` link opened from outside.
    pub fn approve_pair(&mut self, typed: String, window: &mut Window, cx: &mut Context<Self>) {
        let model = self.ctx.model.clone();
        let engine = model.read(cx).engine().clone();
        let sharing = self.ctx.sharing.read(cx);
        let (host_name, username) = match (&sharing.serving, &sharing.account) {
            (Some(_), Some(account)) => ("this device".to_owned(), account.clone()),
            _ => model
                .read(cx)
                .settings()
                .map(|s| (host(&s.remote_url), s.remote_username.clone()))
                .unwrap_or_default(),
        };
        let sheets = self.ctx.sheets.clone();
        self.pairing = Some(cx.spawn_in(window, async move |_, cx| {
            let info = engine.clone().pairing_info(typed.clone()).await;
            cx.update(|window, cx| match info {
                Err(e) => model.update(cx, |m, cx| m.report(describe(&e), cx)),
                Ok(info) => {
                    let origin = if info.local {
                        format!("Requested from {}, on your network.", info.from)
                    } else {
                        format!(
                            "Requested from {}, from the internet. A device in the room with you is usually on your network.",
                            info.from
                        )
                    };
                    let device = info.device.clone();
                    let (allow_model, allow_engine, allow_pair) = (model.clone(), engine.clone(), typed.clone());
                    let (deny_model, deny_engine, deny_pair) = (model.clone(), engine.clone(), typed.clone());
                    let (signed, declined) = (device.clone(), device.clone());
                    let who = username.clone();
                    let sheet = Sheet::new(format!("Sign in {device}?"))
                        .message(format!(
                            "{origin}\n\nIt will be signed in as {username} on {host_name}. Allow only a device you are setting up yourself: anyone can give a device any name."
                        ))
                        .dismiss_with("Decline", move |_, cx| {
                            let (engine, pair, device) = (deny_engine.clone(), deny_pair.clone(), declined.clone());
                            deny_model.update(cx, |m, cx| {
                                m.waiting_pairs.retain(|p| *p != pair);
                                m.run(
                                    format!("Declining {device}"),
                                    crate::settings::model::Uses::NONE,
                                    cx,
                                    move |_| engine.decline_pairing(pair),
                                    move |m, result, cx| {
                                        if let Err(e) = result {
                                            m.report(format!("Could not decline {device}: {}", describe(&e)), cx);
                                        }
                                    },
                                )
                            })
                        })
                        .action(Action::new("Allow", move |_, _, cx| {
                            let (engine, pair, device, who) =
                                (allow_engine.clone(), allow_pair.clone(), signed.clone(), who.clone());
                            allow_model.update(cx, |m, cx| {
                                m.waiting_pairs.retain(|p| *p != pair);
                                m.run(
                                    format!("Signing {device} in"),
                                    crate::settings::model::Uses::NONE,
                                    cx,
                                    move |_| engine.approve_pairing(pair),
                                    move |m, result, cx| match result {
                                        Ok(name) => m.done(format!("{name} is signed in as {who}."), cx),
                                        Err(e) => m.report(format!("Could not sign {device} in: {}", describe(&e)), cx),
                                    },
                                )
                            })
                        }));
                    model.update(cx, |m, _| m.waiting_pairs.push(typed.clone()));
                    sheets.update(cx, |s, cx| s.show(sheet, window, cx));
                }
            })
            .ok();
        }));
    }

    /// Keeping the queue on the server takes the server's queue in place of
    /// this one, so the person is asked first when there is one.
    fn set_server_queue(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        let model = self.ctx.model.clone();
        if !on {
            model.update(cx, |m, cx| m.set_server_queue(false, cx));
            return;
        }
        let engine = model.read(cx).engine().clone();
        let sheets = self.ctx.sheets.clone();
        self.queue_lookup = Some(cx.spawn_in(window, async move |_, cx| {
            let saved = engine.server_queue().await;
            cx.update(|window, cx| match saved {
                Err(e) => model.update(cx, |m, cx| m.report(describe(&e), cx)),
                Ok(ServerQueue { saved_tracks: 0, .. }) => {
                    model.update(cx, |m, cx| m.set_server_queue(true, cx))
                }
                Ok(saved) => {
                    let tracks = if saved.saved_tracks == 1 { "track" } else { "tracks" };
                    let replace = model.clone();
                    let sheet = Sheet::new("Replace this queue?")
                        .message(format!(
                            "Your server has a queue of {} {tracks} saved by {}. Keeping the queue on the server replaces the one on this device with it.",
                            saved.saved_tracks, saved.saved_by
                        ))
                        .action(
                            Action::new("Replace With the Server's Queue", move |_, _, cx| {
                                replace.update(cx, |m, cx| m.set_server_queue(true, cx))
                            })
                            .destructive(),
                        );
                    sheets.update(cx, |s, cx| s.show(sheet, window, cx));
                }
            })
            .ok();
        }));
    }

    fn add_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let typed = self.address.read(cx).value().trim().to_owned();
        if typed.is_empty() {
            return;
        }
        self.address.update(cx, |s, cx| s.set_value("", window, cx));
        self.ctx.model.update(cx, |m, cx| {
            m.edit(cx, move |s| {
                if !s.devices_addresses.contains(&typed) {
                    s.devices_addresses.push(typed.clone());
                }
            })
        });
    }

    /// Lets another account see and control this device, once the person
    /// has seen what that gives it.
    fn grant(&mut self, account: String, window: &mut Window, cx: &mut Context<Self>) {
        if account.is_empty() {
            return;
        }
        let pane = cx.entity();
        let sheet = Sheet::new(format!("Share this device with {account}?"))
            .message(format!(
                "From any network, signed in as themselves, {account} will be able to see and control what this device plays, including the output, preset and volume. Nothing of your library, playlists, history or settings."
            ))
            .action(Action::new("Share", move |_, window, cx| {
                let account = account.clone();
                pane.update(cx, |p, cx| p.share(account, true, window, cx))
            }));
        self.ctx
            .sheets
            .update(cx, |s, cx| s.show(sheet, window, cx));
    }

    fn share(&mut self, account: String, allow: bool, window: &mut Window, cx: &mut Context<Self>) {
        if account.is_empty() {
            return;
        }
        let engine = self.ctx.model.read(cx).engine().clone();
        // Ours if it could not be sent; the server's, if it refused, arrives
        // in the connection.
        match engine.share_device(account, allow) {
            Ok(()) => {
                self.share_error = None;
                if allow {
                    self.grantee.update(cx, |s, cx| s.set_value("", window, cx));
                }
            }
            Err(e) => self.share_error = Some(describe(&e)),
        }
        cx.notify();
    }

    fn confirm_stop_sharing(&self, account: String, window: &mut Window, cx: &mut Context<Self>) {
        let pane = cx.entity();
        let sheet = Sheet::new(format!("Stop sharing with {account}?"))
            .message("They can no longer see or control what this device plays.")
            .action(
                Action::new("Stop Sharing", move |_, window, cx| {
                    let account = account.clone();
                    pane.update(cx, |pane, cx| pane.share(account, false, window, cx))
                })
                .destructive(),
            );
        self.ctx
            .sheets
            .update(cx, |s, cx| s.show(sheet, window, cx));
    }

    /// Hangs up a device connected on the network; refusing it as well
    /// keeps its address out for good.
    fn end(
        &self,
        connection: ConnectedInfo,
        refuse: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let model = self.ctx.model.clone();
        let (title, message, label) = if refuse {
            (
                format!("Refuse {}?", connection.name),
                "It is hung up, and its address is kept out until you allow it here.",
                "Refuse",
            )
        } else {
            (
                format!("Disconnect {}?", connection.name),
                "It is hung up and kept from connecting again until you allow it back here, play on it, pick it in Play on, or kōan restarts.",
                "Disconnect",
            )
        };
        let sheet = Sheet::new(title).message(message).action(
            Action::new(label, move |_, _, cx| {
                let connection = connection.clone();
                model.update(cx, |m, cx| {
                    if refuse && let Some(addr) = connection.addr.clone() {
                        m.edit(cx, move |s| {
                            if !s.devices_refused.contains(&addr) {
                                s.devices_refused.push(addr.clone());
                            }
                        });
                    }
                    if let Some(key) = connection.key {
                        m.engine().end_connection(key);
                    }
                })
            })
            .destructive(),
        );
        self.ctx
            .sheets
            .update(cx, |s, cx| s.show(sheet, window, cx));
    }
}

/// What a connection is, and since when.
fn detail(c: &ConnectedInfo) -> String {
    let who = if c.own {
        "your device".to_owned()
    } else if let Some(owner) = &c.owner {
        format!("shared by {owner}")
    } else {
        "not proved".to_owned()
    };
    let what = match (c.via_server, c.inbound) {
        (true, false) => "Your server".to_owned(),
        (true, true) => "Your device, through the server".to_owned(),
        (false, true) => format!("Can control this device on this network, {who}"),
        (false, false) => format!("This device is connected to it on this network, {who}"),
    };
    match c.since {
        Some(since) => format!("{what} · since {}", moment(since)),
        None => what,
    }
}

impl Render for DevicesPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let Some(settings) = self.ctx.model.read(cx).settings().cloned() else {
            return page("devices", &look, Vec::new());
        };
        let connection = self.ctx.connection.read(cx).0.clone();
        let entity = cx.entity();
        let model = self.ctx.model.clone();
        let mut sections = Vec::new();

        if (settings.remote_signed_in || self.ctx.sharing.read(cx).serving.is_some())
            && connection.as_ref().is_some_and(|c| c.pairing)
        {
            {
                let ready = !self.code.read(cx).value().trim().is_empty();
                let pane = entity.clone();
                sections.push(section(
                    Some("Pair a device"),
                    vec![crate::components::form_entry(
                        TextField::new(&self.code).into_any_element(),
                        vec![
                            Button::new("approve-code", ButtonKind::Prominent)
                                .label("Approve")
                                .disabled(!ready)
                                .on_click(move |_, window, cx| pane.update(cx, |p, cx| p.approve(window, cx)))
                                .into_any_element(),
                        ],
                        &look,
                    )],
                    Some("A television or another device without a keyboard shows a code while it waits. Enter it here to sign it in as you, with a key of its own that can be revoked on the server.".into()),
                    &look,
                ));
            }
        }
        if settings.remote_signed_in {
            let pane = entity.clone();
            sections.push(section(
                Some("Play queue"),
                vec![Toggle::new("server-queue", "Keep the queue on the server", settings.play_queue)
                    .on_change(move |on, window, cx| {
                        let on = *on;
                        pane.update(cx, |p, cx| p.set_server_queue(on, window, cx))
                    })
                    .into_any_element()],
                Some("Saves this device's queue to your account on the server, where other apps can pick it up, and picks up a queue another app saved there when kōan starts. Moving music between kōan devices does not need it.".into()),
                &look,
            ));
        }

        let mut this_device = vec![
            {
                let model = model.clone();
                Toggle::new(
                    "discoverable",
                    "Discoverable on this network",
                    settings.devices_discoverable,
                )
                .on_change(move |on, _, cx| {
                    let on = *on;
                    model.update(cx, |m, cx| m.edit(cx, move |s| s.devices_discoverable = on))
                })
                .into_any_element()
            },
            {
                let model = model.clone();
                Row::new("Devices on this network")
                    .control(
                        Picker::new(
                            "nearby-control",
                            NEARBY.map(|(label, _)| label),
                            NEARBY
                                .iter()
                                .position(|(_, v)| *v == settings.devices_nearby_control),
                        )
                        .on_change(move |index, _, cx| {
                            let value = NEARBY[*index].1;
                            model.update(cx, |m, cx| {
                                m.edit(cx, move |s| s.devices_nearby_control = value.into())
                            })
                        }),
                    )
                    .into_any_element()
            },
        ];
        if let Some(port) = connection.as_ref().and_then(|c| c.listening_port) {
            this_device.push(
                Row::new("Listening on port")
                    .value(port.to_string())
                    .into_any_element(),
            );
        }
        if connection.as_ref().is_some_and(|c| c.local_network_blocked) {
            this_device.push(note(
                "Blocked. Allow Local Network for kōan in System Settings → Privacy & Security.",
                Icon::NetworkBlocked,
                Tone::Bad,
                &look,
            ));
        }
        sections.push(section(
            Some("This device"),
            this_device,
            Some("Other kōan apps on this network can see and control what plays here. Full control adds the output, preset, volume and moving the music; Playback only is play and the queue. Neither reaches your library. On a network shared with strangers, choose Playback only.".into()),
            &look,
        ));

        // Who is connected, each with a way to end it where this device can.
        let connections = connection
            .as_ref()
            .map(|c| c.connections.clone())
            .unwrap_or_default();
        let held = connection
            .as_ref()
            .map(|c| c.held.clone())
            .unwrap_or_default();
        let refused = settings.devices_refused.clone();
        if !connections.is_empty() || !held.is_empty() || !refused.is_empty() {
            let mut rows = Vec::new();
            for (index, c) in connections.into_iter().enumerate() {
                let mut controls = Vec::new();
                if c.key.is_some() {
                    if !c.own && c.owner.is_none() && c.addr.is_some() {
                        let (pane, c) = (entity.clone(), c.clone());
                        controls.push(
                            Button::new(("refuse", index), ButtonKind::Compact)
                                .label("Refuse…")
                                .destructive(true)
                                .on_click(move |_, window, cx| {
                                    let c = c.clone();
                                    pane.update(cx, |p, cx| p.end(c, true, window, cx))
                                })
                                .into_any_element(),
                        );
                    }
                    let (pane, c2) = (entity.clone(), c.clone());
                    controls.push(
                        Button::new(("disconnect", index), ButtonKind::Compact)
                            .label("Disconnect…")
                            .on_click(move |_, window, cx| {
                                let c = c2.clone();
                                pane.update(cx, |p, cx| p.end(c, false, window, cx))
                            })
                            .into_any_element(),
                    );
                }
                rows.push(item(
                    c.name.clone(),
                    Some(detail(&c).into()),
                    controls,
                    &look,
                ));
            }
            for (index, h) in held.into_iter().enumerate() {
                let engine = model.read(cx).engine().clone();
                let addr = h.addr.clone();
                rows.push(item(
                    h.name.clone().unwrap_or_else(|| h.addr.clone()),
                    Some("Disconnected until allowed back".into()),
                    vec![
                        Button::new(("allow-held", index), ButtonKind::Compact)
                            .label("Allow")
                            .on_click(move |_, _, _| engine.allow_held(addr.clone()))
                            .into_any_element(),
                    ],
                    &look,
                ));
            }
            for (index, addr) in refused.into_iter().enumerate() {
                let model = model.clone();
                let allowed = addr.clone();
                rows.push(item(
                    addr,
                    Some("Refused".into()),
                    vec![
                        Button::new(("allow-refused", index), ButtonKind::Compact)
                            .label("Allow")
                            .on_click(move |_, _, cx| {
                                let allowed = allowed.clone();
                                model.update(cx, |m, cx| {
                                    m.edit(cx, move |s| s.devices_refused.retain(|a| *a != allowed))
                                })
                            })
                            .into_any_element(),
                    ],
                    &look,
                ));
            }
            let through_server = if connection.as_ref().is_some_and(|c| c.devices) {
                " Your account's devices reach this one through the server for as long as they are signed in; revoke one under Account."
            } else {
                ""
            };
            sections.push(section(
                Some("Connected now"),
                rows,
                Some(format!("A device that can control this one uses its battery while it does. Disconnect hangs up a device on this network until you allow it back. Refuse keeps a device that has not proved it is yours out for good, by its address.{through_server}").into()),
                &look,
            ));
        }

        let mut addresses: Vec<AnyElement> = settings
            .devices_addresses
            .iter()
            .enumerate()
            .map(|(index, addr)| {
                let model = model.clone();
                let removed = addr.clone();
                item(
                    addr.clone(),
                    None,
                    vec![
                        Button::new(("remove-address", index), ButtonKind::Compact)
                            .label("Remove")
                            .destructive(true)
                            .on_click(move |_, _, cx| {
                                let removed = removed.clone();
                                model.update(cx, |m, cx| {
                                    m.edit(cx, move |s| {
                                        s.devices_addresses.retain(|a| *a != removed)
                                    })
                                })
                            })
                            .into_any_element(),
                    ],
                    &look,
                )
            })
            .collect();
        let ready = !self.address.read(cx).value().trim().is_empty();
        let pane = entity.clone();
        addresses.push(buttons(
            vec![
                div()
                    .w(px(260.))
                    .child(TextField::new(&self.address))
                    .into_any_element(),
                Button::new("add-address", ButtonKind::Bordered)
                    .label("Add")
                    .disabled(!ready)
                    .on_click(move |_, window, cx| {
                        pane.update(cx, |p, cx| p.add_address(window, cx))
                    })
                    .into_any_element(),
            ],
            &look,
        ));
        sections.push(section(
            Some("Devices by address"),
            addresses,
            Some("For networks that do not announce devices, such as a tailnet. The port is 5626 unless the other device says otherwise.".into()),
            &look,
        ));

        if let Some(c) = connection.as_ref().filter(|c| c.sharing) {
            let mut rows: Vec<AnyElement> = c
                .shared_with
                .iter()
                .enumerate()
                .map(|(index, account)| {
                    let (pane, account2) = (entity.clone(), account.clone());
                    item(
                        account.clone(),
                        None,
                        vec![
                            Button::new(("stop-sharing", index), ButtonKind::Compact)
                                .label("Stop Sharing…")
                                .destructive(true)
                                .on_click(move |_, window, cx| {
                                    let account = account2.clone();
                                    pane.update(cx, |p, cx| {
                                        p.confirm_stop_sharing(account, window, cx)
                                    })
                                })
                                .into_any_element(),
                        ],
                        &look,
                    )
                })
                .collect();
            let typed = self.grantee.read(cx).value().trim().to_lowercase();
            let shared: Vec<&String> = c.shared_with.iter().collect();
            // The server's accounts matching what is typed, not shared with yet.
            let suggestions: Vec<String> = c
                .share_accounts
                .iter()
                .filter(|a| {
                    !shared.contains(a)
                        && !typed.is_empty()
                        && a.to_lowercase().starts_with(&typed)
                        && a.to_lowercase() != typed
                })
                .take(5)
                .cloned()
                .collect();
            let pane = entity.clone();
            rows.push(buttons(
                vec![
                    div()
                        .w(px(260.))
                        .child(TextField::new(&self.grantee))
                        .into_any_element(),
                    Button::new("share-device", ButtonKind::Bordered)
                        .label("Share")
                        .disabled(typed.is_empty())
                        .on_click(move |_, window, cx| {
                            pane.update(cx, |p, cx| {
                                let name = p.grantee.read(cx).value().trim().to_owned();
                                p.grant(name, window, cx)
                            })
                        })
                        .into_any_element(),
                ],
                &look,
            ));
            for (index, account) in suggestions.into_iter().enumerate() {
                let grantee = self.grantee.clone();
                let chosen = account.clone();
                rows.push(
                    Button::new(("suggest", index), ButtonKind::Text)
                        .label(account)
                        .on_click(move |_, window, cx| {
                            let chosen = chosen.clone();
                            grantee.update(cx, |s, cx| s.set_value(chosen, window, cx))
                        })
                        .into_any_element(),
                );
            }
            if let Some(error) = self.share_error.clone().or_else(|| c.share_error.clone()) {
                rows.push(text(error, TypeRole::Fine, Tone::Bad, &look).into_any_element());
            }
            sections.push(section(
                Some("Shared with other accounts"),
                rows,
                Some("From any network, as their own account, they can see and control what this device plays, including the output, preset and volume. Nothing of your library, playlists, history or settings.".into()),
                &look,
            ));
        }
        page("devices", &look, sections)
    }
}

#[cfg(test)]
mod tests {
    use super::{ConnectedInfo, detail};

    fn connection(via_server: bool, inbound: bool, own: bool) -> ConnectedInfo {
        ConnectedInfo {
            key: None,
            via_server,
            inbound,
            name: "Den".into(),
            addr: None,
            own,
            owner: None,
            since: None,
        }
    }

    #[test]
    fn says_what_a_connection_is() {
        assert_eq!(detail(&connection(true, false, false)), "Your server");
        assert_eq!(
            detail(&connection(true, true, true)),
            "Your device, through the server"
        );
        assert_eq!(
            detail(&connection(false, true, true)),
            "Can control this device on this network, your device"
        );
        let mut shared = connection(false, false, false);
        shared.owner = Some("sam".into());
        assert_eq!(
            detail(&shared),
            "This device is connected to it on this network, shared by sam"
        );
    }
}
