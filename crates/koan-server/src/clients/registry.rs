//! The linked devices themselves: registering and dropping links, their
//! reported state, the account's devices as each is told them, levels, and
//! devices shared between accounts.

use super::*;

impl Registry {
    /// Add a client, replacing any earlier connection from the same device
    /// and account. Returns its id.
    #[allow(clippy::too_many_arguments)]
    pub fn register(
        &self,
        username: &str,
        name: &str,
        platform: &str,
        device: &str,
        tx: LinkTx,
        wants_devices: bool,
        acks: bool,
    ) -> (String, Vec<Envelope>) {
        self.link(
            username,
            name,
            platform,
            device,
            tx,
            wants_devices,
            acks,
            true,
        )
    }

    /// `register` for a link that gave no device id: one never seen again
    /// under the id it was given, so it is not recorded as a device, and
    /// nothing is kept for it once it closes.
    #[allow(clippy::too_many_arguments)]
    pub fn register_passing(
        &self,
        username: &str,
        name: &str,
        platform: &str,
        device: &str,
        tx: LinkTx,
        wants_devices: bool,
        acks: bool,
    ) -> (String, Vec<Envelope>) {
        self.link(
            username,
            name,
            platform,
            device,
            tx,
            wants_devices,
            acks,
            false,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn link(
        &self,
        username: &str,
        name: &str,
        platform: &str,
        device: &str,
        tx: LinkTx,
        wants_devices: bool,
        acks: bool,
        remember: bool,
    ) -> (String, Vec<Envelope>) {
        let id = uuid::Uuid::now_v7().to_string();
        if let Some((sent, how)) = WOKEN
            .lock()
            .remove(&(device.to_string(), username.to_string()))
        {
            log::info!(
                "wake: {name} linked {}ms after its {how}",
                sent.elapsed().as_millis()
            );
        }
        // What waited for this device while it was away is handed back for
        // the session to write before anything else: it is not bounded by
        // the link's queue, and the entry, which takes live commands, is
        // added only after it is taken.
        let mut waiting = Vec::new();
        if remember {
            let live = self.live();
            let pruned;
            (waiting, pruned) = outbox::take_and_remember(username, device, name, platform, &live);
            // Devices forgotten for age take their addresses with them.
            if pruned {
                let kept = outbox::load_addresses();
                self.addresses
                    .lock()
                    .retain(|device, _| kept.contains_key(device));
            }
        }
        let mut entries = self.entries.lock();
        entries.retain(|e| !(e.device == device && e.info.username == username));
        entries.push(Entry {
            info: ClientInfo {
                id: id.clone(),
                device: device.to_string(),
                name: name.to_string(),
                platform: platform.to_string(),
                username: username.to_string(),
                connected_at: chrono::Utc::now().timestamp(),
                state: LinkState::default(),
                last_played_at: None,
                state_at: chrono::Utc::now().timestamp_millis(),
                reports: false,
                notified: false,
                acks,
            },
            device: device.to_string(),
            tx,
            wants_devices,
            wants_keys: false,
        });
        drop(entries);
        // Always, empty or not: an app signed in elsewhere before holds that
        // server's list.
        self.send_shares(username, device, None);
        self.announce(username);
        // A device that relinked has a new session, which holds no watch:
        // tell it again that it is watched, if it is.
        if self
            .level_watches
            .lock()
            .iter()
            .any(|w| w.owner == username && w.target == device)
        {
            self.send_live(username, device, LinkCommand::WatchLevels { on: true });
        }
        (id, waiting)
    }

    /// Send the link `id` the account's device keys from now on.
    pub fn wants_keys(&self, id: &str) {
        if let Some(e) = self.entries.lock().iter_mut().find(|e| e.info.id == id) {
            e.wants_keys = true;
        }
    }

    /// Hand `username`'s links that take them the device keys they check the
    /// local network against.
    pub fn publish_keys(&self, username: &str, keys: Vec<koan_core::remote::link::LinkDeviceKey>) {
        for e in self
            .entries
            .lock()
            .iter()
            .filter(|e| e.info.username == username && e.wants_keys)
        {
            let _ = e.tx.send(LinkCommand::DeviceKeys { keys: keys.clone() });
        }
    }

    /// Every account with a link that takes device keys.
    pub fn keyed_accounts(&self) -> Vec<String> {
        let mut accounts: Vec<String> = self
            .entries
            .lock()
            .iter()
            .filter(|e| e.wants_keys)
            .map(|e| e.info.username.clone())
            .collect();
        accounts.sort();
        accounts.dedup();
        accounts
    }

    /// The accounts `owner` shares `device` with.
    pub fn grantees_of(&self, owner: &str, device: &str) -> Vec<String> {
        self.grants
            .lock()
            .iter()
            .filter(|g| g.owner == owner && g.device == device)
            .map(|g| g.grantee.clone())
            .collect()
    }

    /// Record what a client says it is doing.
    pub fn report(&self, id: &str, state: LinkState) {
        let mut entries = self.entries.lock();
        if let Some(e) = entries.iter_mut().find(|e| e.info.id == id) {
            if state.playing || e.info.state.playing {
                e.info.last_played_at = Some(chrono::Utc::now().timestamp());
            }
            e.info.state = state;
            e.info.state_at = chrono::Utc::now().timestamp_millis();
            e.info.reports = true;
            let (username, device) = (e.info.username.clone(), e.device.clone());
            drop(entries);
            self.announce(&username);
            self.update_activities(&username, &device);
        }
    }

    /// Record the relay's handle for this device.
    pub fn set_push(&self, username: &str, device: &str, handle: &str) {
        outbox::save_push(username, device, handle);
    }

    /// Keep `pairing` for `username`'s Apple TV `device`, or drop the one
    /// kept, and tell the account's devices. Only a TV the account has
    /// linked is taken.
    pub fn set_tv_pairing(
        &self,
        username: &str,
        device: &str,
        pairing: Option<&koan_core::remote::companion::Credential>,
    ) -> Result<(), String> {
        if pairing.is_some() && !outbox::is_tv(username, device) {
            return Err(format!("{device} is not one of {username}'s Apple TVs"));
        }
        outbox::save_tv_pairing(username, device, pairing);
        log::info!(
            "devices: {username} {} the Apple TV {device}",
            if pairing.is_some() {
                "paired"
            } else {
                "unpaired"
            }
        );
        self.announce(username);
        Ok(())
    }

    /// Close every link `username` has open. Dropping an entry's sender ends
    /// its session, and the client must then sign in again to reconnect.
    pub fn disconnect(&self, username: &str) {
        self.entries.lock().retain(|e| e.info.username != username);
    }

    pub fn unregister(&self, id: &str) {
        let mut entries = self.entries.lock();
        let gone = entries
            .iter()
            .find(|e| e.info.id == id)
            .map(|e| (e.device.clone(), e.info.username.clone()));
        entries.retain(|e| e.info.id != id);
        drop(entries);
        if let Some((device, username)) = gone {
            outbox::touch(&[(device.clone(), username.clone())]);
            self.announce(&username);
            // A controller that went stops watching whatever it watched.
            let targets: Vec<String> = self
                .level_watches
                .lock()
                .iter()
                .filter(|w| w.watcher_user == username && w.watcher == device)
                .map(|w| w.target.clone())
                .collect();
            for target in targets {
                self.watch_levels(&username, &device, &target, false);
            }
        }
    }

    /// `watcher` has `target`'s playing bars on screen, or no longer has. The
    /// target is told to send its levels while anyone watches, and to stop
    /// when the last one goes. Only over a live link: levels are no reason to
    /// wake a phone or to queue a command for later.
    pub fn watch_levels(&self, username: &str, watcher: &str, target: &str, on: bool) {
        // A device shared with `username` is watched as its owner's.
        let owner = self
            .shared_owner(username, target)
            .unwrap_or_else(|| username.to_string());
        let mut watches = self.level_watches.lock();
        watches.retain(|w| {
            !(w.watcher_user == username && w.watcher == watcher && w.target == target)
        });
        if on {
            watches.push(LevelWatch {
                owner: owner.clone(),
                watcher_user: username.into(),
                watcher: watcher.into(),
                target: target.into(),
            });
        }
        let watched = watches
            .iter()
            .any(|w| w.owner == owner && w.target == target);
        drop(watches);
        if on || !watched {
            self.send_live(&owner, target, LinkCommand::WatchLevels { on: watched });
        }
    }

    /// A frame of `from`'s levels, for each device watching it. Not stored:
    /// one that cannot be delivered now is of no use later.
    pub fn levels(&self, username: &str, from: &str, f: koan_core::remote::levels::Frame) {
        let watchers: Vec<(String, String)> = self
            .level_watches
            .lock()
            .iter()
            .filter(|w| w.owner == username && w.target == from)
            .map(|w| (w.watcher_user.clone(), w.watcher.clone()))
            .collect();
        let entries = self.entries.lock();
        for (watcher_user, watcher) in watchers {
            if let Some(e) = entries
                .iter()
                .find(|e| e.info.username == watcher_user && e.device == watcher)
            {
                e.tx.send_lossy(LinkCommand::Levels {
                    from: from.to_string(),
                    f,
                });
            }
        }
    }

    /// Send `envelope` down `device`'s link as it stands, ack id and all, as
    /// its outbox is when it links: a command kept for it while it was away.
    pub(crate) fn send_kept(&self, username: &str, device: &str, envelope: Envelope) {
        if let Some(e) = self
            .entries
            .lock()
            .iter()
            .find(|e| e.info.username == username && e.device == device)
        {
            let _ = e.tx.send(envelope);
        }
    }

    /// Send `cmd` to `device` if it is linked now, and nowhere else.
    pub(crate) fn send_live(&self, username: &str, device: &str, cmd: LinkCommand) {
        if let Some(e) = self
            .entries
            .lock()
            .iter()
            .find(|e| e.info.username == username && e.device == device)
        {
            let _ = e.tx.send(cmd);
        }
    }

    /// Send each of `username`'s links that asked for them the account's
    /// other devices: those linked, with what each is doing, and those a push
    /// can wake.
    pub(super) fn announce(&self, username: &str) {
        self.announce_to(username);
        // Those `username` shares a device with see it change too.
        let grantees: Vec<String> = {
            let mut g: Vec<String> = self
                .grants
                .lock()
                .iter()
                .filter(|g| g.owner == username)
                .map(|g| g.grantee.clone())
                .collect();
            g.sort();
            g.dedup();
            g
        };
        for grantee in grantees {
            self.announce_to(&grantee);
        }
    }

    /// `announce` for `username` alone.
    pub(super) fn announce_to(&self, username: &str) {
        let listening = |e: &Entry| e.info.username == username && e.wants_devices;
        if !self.entries.lock().iter().any(listening) {
            return;
        }
        let asleep = outbox::push_targets(Some(username));
        let tvs = outbox::tv_pairings(username);
        // Waking takes a push key as well as the device's token.
        let can_push = crate::push::pusher().is_some();
        // Read before the entries are held: every link waits on them.
        let grants: Vec<Grant> = self
            .grants
            .lock()
            .iter()
            .filter(|g| g.grantee == username)
            .cloned()
            .collect();
        let mut owners: Vec<&str> = grants.iter().map(|g| g.owner.as_str()).collect();
        owners.sort();
        owners.dedup();
        let owners_asleep: Vec<outbox::PushTarget> = owners
            .into_iter()
            .flat_map(|owner| outbox::push_targets(Some(owner)))
            .collect();
        let entries = self.entries.lock();
        let ours: Vec<&Entry> = entries
            .iter()
            .filter(|e| e.info.username == username)
            .collect();
        if !ours.iter().any(|e| e.wants_devices) {
            return;
        }
        let mut all: Vec<LinkDevice> = ours
            .iter()
            .map(|e| LinkDevice {
                id: e.device.clone(),
                name: e.info.name.clone(),
                platform: e.info.platform.clone(),
                linked: true,
                state: e.info.reports.then(|| LinkState {
                    position_ms: e.info.position_ms(),
                    ..e.info.state.clone()
                }),
                last_seen: None,
                wakeable: Some(can_push && asleep.iter().any(|t| t.device == e.device)),
                owner: None,
                acks: e.info.acks,
                tv_pairing: tvs
                    .iter()
                    .find(|t| t.device == e.device)
                    .map(|t| t.pairing.clone()),
            })
            .collect();
        for t in asleep {
            if !all.iter().any(|d| d.id == t.device) {
                all.push(LinkDevice {
                    tv_pairing: tvs
                        .iter()
                        .find(|p| p.device == t.device)
                        .map(|p| p.pairing.clone()),
                    id: t.device,
                    name: t.name,
                    platform: t.platform,
                    linked: false,
                    state: None,
                    last_seen: Some(t.last_seen),
                    wakeable: Some(can_push),
                    owner: None,
                    acks: false,
                });
            }
        }
        // A paired Apple TV is woken by the account's devices, push or no.
        for t in tvs {
            if !all.iter().any(|d| d.id == t.device) {
                all.push(LinkDevice {
                    id: t.device,
                    name: t.name,
                    platform: t.platform,
                    linked: false,
                    state: None,
                    last_seen: Some(t.last_seen),
                    wakeable: Some(false),
                    owner: None,
                    acks: false,
                    tv_pairing: Some(t.pairing),
                });
            }
        }
        all.extend(Self::shared_devices(
            grants,
            &owners_asleep,
            &entries,
            can_push,
        ));
        for e in ours.iter().filter(|e| e.wants_devices) {
            let devices = all.iter().filter(|d| d.id != e.device).cloned().collect();
            let _ = e.tx.send(LinkCommand::Devices { devices });
        }
    }

    /// The devices shared with `grantee`, as `grantee` sees them: their
    /// state, with nothing of their owner's own (the outputs are the owner's
    /// to choose); and absent ones only where a push can wake them.
    pub(super) fn shared_devices(
        grants: Vec<Grant>,
        owners_asleep: &[outbox::PushTarget],
        entries: &[Entry],
        can_push: bool,
    ) -> Vec<LinkDevice> {
        let mut out = Vec::new();
        for g in grants {
            let linked = entries
                .iter()
                .find(|e| e.device == g.device && e.info.username == g.owner);
            match linked {
                Some(e) => out.push(LinkDevice {
                    id: e.device.clone(),
                    name: e.info.name.clone(),
                    platform: e.info.platform.clone(),
                    linked: true,
                    state: e.info.reports.then(|| LinkState {
                        position_ms: e.info.position_ms(),
                        ..e.info.state.clone()
                    }),
                    last_seen: None,
                    wakeable: Some(can_push),
                    owner: Some(g.owner.clone()),
                    acks: e.info.acks,
                    tv_pairing: None,
                }),
                None => {
                    if let Some(t) = owners_asleep
                        .iter()
                        .find(|t| t.username == g.owner && t.device == g.device)
                        .cloned()
                    {
                        out.push(LinkDevice {
                            id: t.device,
                            name: t.name,
                            platform: t.platform,
                            linked: false,
                            state: None,
                            last_seen: Some(t.last_seen),
                            wakeable: Some(can_push),
                            owner: Some(g.owner.clone()),
                            acks: false,
                            tv_pairing: None,
                        });
                    }
                }
            }
        }
        out
    }

    /// Let `grantee` control `owner`'s device `device`, or stop letting it.
    /// Asked by the device itself; the caller has checked `grantee` is an
    /// account on this server.
    pub fn share(
        &self,
        owner: &str,
        device: &str,
        grantee: &str,
        allow: bool,
    ) -> Result<(), String> {
        if grantee == owner {
            return Err("a device is already its own account's".into());
        }
        let grant = Grant {
            device: device.to_string(),
            owner: owner.to_string(),
            grantee: grantee.to_string(),
        };
        {
            let mut grants = self.grants.lock();
            grants.retain(|g| *g != grant);
            if allow {
                grants.push(grant.clone());
            }
        }
        if allow {
            outbox::save_grant(&grant);
        } else {
            outbox::drop_grant(&grant);
        }
        log::info!(
            "share: {owner}'s {device} {} {grantee}",
            if allow {
                "shared with"
            } else {
                "no longer shared with"
            }
        );
        self.send_shares(owner, device, None);
        self.announce_to(grantee);
        Ok(())
    }

    /// Tell `owner`'s device `device` whom it is shared with, and why its last
    /// request was refused, if it was.
    pub fn send_shares(&self, owner: &str, device: &str, error: Option<String>) {
        let mut grantees: Vec<String> = self
            .grants
            .lock()
            .iter()
            .filter(|g| g.owner == owner && g.device == device)
            .map(|g| g.grantee.clone())
            .collect();
        grantees.sort();
        let listening =
            |e: &Entry| e.info.username == owner && e.device == device && e.wants_devices;
        if !self.entries.lock().iter().any(listening) {
            return;
        }
        // Read before the entries are held: every link waits on them.
        let accounts = outbox::accounts()
            .into_iter()
            .filter(|a| a != owner)
            .collect();
        if let Some(e) = self.entries.lock().iter().find(|e| listening(e)) {
            let _ = e.tx.send(LinkCommand::Shares {
                grantees,
                error,
                accounts,
            });
        }
    }

    /// `device`, of `username`'s, linked from `addr`.
    pub fn seen_at(&self, device: &str, username: &str, addr: std::net::IpAddr) {
        self.addresses
            .lock()
            .insert(device.to_string(), (username.to_string(), addr));
        outbox::save_address(device, username, addr);
    }

    /// Whose push token wakes `to` when `username`'s device `from` asks: the
    /// owner of a device shared with `username`; the account of one last seen
    /// at the same address as `from` is now, which is to say behind the same
    /// router, a household's; else `username`'s own. A wake grants nothing
    /// else: what may then be done to the device follows the share or the
    /// network as ever.
    pub(super) fn wake_owner(&self, username: &str, from: &str, to: &str) -> String {
        if let Some(owner) = self.shared_owner(username, to) {
            return owner;
        }
        let addresses = self.addresses.lock();
        let here = addresses
            .get(from)
            .filter(|(user, _)| user == username)
            .map(|(_, addr)| *addr);
        match (here, addresses.get(to)) {
            (Some(here), Some((owner, there))) if *there == here => owner.clone(),
            _ => username.to_string(),
        }
    }

    /// The account `to` belongs to, when `from`, a device of `owner`'s, is
    /// shared with it.
    pub(super) fn grantee_of(&self, owner: &str, from: &str, to: &str) -> Option<String> {
        let to_user = self
            .entries
            .lock()
            .iter()
            .find(|e| e.device == to && e.info.username != owner)
            .map(|e| e.info.username.clone())?;
        self.grants
            .lock()
            .iter()
            .any(|g| g.owner == owner && g.device == from && g.grantee == to_user)
            .then_some(to_user)
    }

    /// Whose device `to` is, when it is not `username`'s own but shared with
    /// it: the owner.
    pub(super) fn shared_owner(&self, username: &str, to: &str) -> Option<String> {
        self.grants
            .lock()
            .iter()
            .find(|g| g.grantee == username && g.device == to)
            .map(|g| g.owner.clone())
    }
}

impl Registry {
    /// Forget `device`, one of `username`'s that is not linked: what the
    /// server keeps of it, its push token included, so nothing is pushed to
    /// it again; and every linked device of the account drops it. Another
    /// account's device is out of reach of this, by construction: every
    /// record is keyed by account. A device that links again is recorded
    /// afresh.
    pub fn forget(&self, username: &str, device: &str) -> Result<(), String> {
        // Another account's device, shared with this one: forgetting it
        // declines the share, or the next list would bring it straight back.
        if let Some(owner) = self.shared_owner(username, device) {
            return self.share(&owner, device, username, false);
        }
        if self.list(Some(username)).iter().any(|c| c.device == device) {
            return Err(format!("{device} is linked; it would be back at once"));
        }
        outbox::forget_device(device, username);
        log::info!("devices: {username} forgot {device}");
        self.broadcast(
            Some(username),
            LinkCommand::Forgotten {
                device: device.to_string(),
            },
        );
        self.announce(username);
        Ok(())
    }
}

impl Registry {
    /// Clients `username` may command, newest first; every client for `None`.
    pub fn list(&self, username: Option<&str>) -> Vec<ClientInfo> {
        let mut out: Vec<ClientInfo> = self
            .entries
            .lock()
            .iter()
            .filter(|e| username.is_none_or(|u| e.info.username == u))
            .map(|e| e.info.clone())
            .collect();
        out.sort_by_key(|c| std::cmp::Reverse(c.connected_at));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clients::testing::*;

    #[test]
    fn a_watch_is_never_queued_and_is_renewed_when_the_target_relinks() {
        let reg = Registry::default();
        let watches = |rx: &mut tokio::sync::mpsc::Receiver<Envelope>| {
            std::iter::from_fn(|| rx.try_recv().ok().map(|e| e.command))
                .filter(|c| matches!(c, LinkCommand::WatchLevels { .. }))
                .collect::<Vec<_>>()
        };
        // Not as a relayed command: that path queues and pushes.
        assert!(
            reg.relay("rl", "rl-phone", LinkCommand::WatchLevels { on: true })
                .is_err()
        );
        // Watched while away: nothing is queued for it.
        reg.watch_levels("rl", "rl-mac", "rl-phone", true);
        let (tx, mut phone) = super::LinkTx::channel();
        reg.register("rl", "phone", "ios", "rl-phone", tx, false, false);
        assert_eq!(
            watches(&mut phone),
            vec![LinkCommand::WatchLevels { on: true }],
            "told once, on linking, because it is watched now"
        );
        // Relinked: a new session, told again.
        let (tx, mut phone) = super::LinkTx::channel();
        reg.register("rl", "phone", "ios", "rl-phone", tx, false, false);
        assert_eq!(
            watches(&mut phone),
            vec![LinkCommand::WatchLevels { on: true }]
        );
    }

    #[test]
    fn levels_reach_a_watcher_only_while_it_watches() {
        use koan_core::remote::levels::Frame;
        let reg = Registry::default();
        let (tx_mac, mut mac) = super::LinkTx::channel();
        let (tx_phone, mut phone) = super::LinkTx::channel();
        let mac_id = reg
            .register("lv", "mac", "macos", "lv-mac", tx_mac, false, false)
            .0;
        reg.register("lv", "phone", "ios", "lv-phone", tx_phone, false, false);
        let levels = |rx: &mut tokio::sync::mpsc::Receiver<Envelope>| {
            std::iter::from_fn(|| rx.try_recv().ok().map(|e| e.command))
                .filter(|c| {
                    matches!(
                        c,
                        LinkCommand::Levels { .. } | LinkCommand::WatchLevels { .. }
                    )
                })
                .collect::<Vec<_>>()
        };
        let f = Frame(1_000, 1, 2, 3);

        reg.levels("lv", "lv-phone", f);
        assert!(levels(&mut mac).is_empty(), "nobody watching");

        reg.watch_levels("lv", "lv-mac", "lv-phone", true);
        assert_eq!(
            levels(&mut phone),
            vec![LinkCommand::WatchLevels { on: true }]
        );
        reg.levels("lv", "lv-phone", f);
        assert_eq!(
            levels(&mut mac),
            vec![LinkCommand::Levels {
                from: "lv-phone".into(),
                f
            }]
        );

        // The watcher's link goes: the phone is told to stop.
        reg.unregister(&mac_id);
        assert_eq!(
            levels(&mut phone),
            vec![LinkCommand::WatchLevels { on: false }]
        );
        reg.watch_levels("lv", "lv-mac", "lv-phone", true);
        reg.watch_levels("lv", "lv-mac", "lv-phone", false);
        assert_eq!(
            levels(&mut phone),
            vec![
                LinkCommand::WatchLevels { on: true },
                LinkCommand::WatchLevels { on: false }
            ]
        );
    }

    #[test]
    fn a_reconnect_replaces_the_device_and_commands_reach_it() {
        let reg = Registry::default();
        let (tx1, _rx1) = super::LinkTx::channel();
        let (tx2, mut rx2) = super::LinkTx::channel();
        let (tx3, _rx3) = super::LinkTx::channel();
        reg.register("j", "phone", "ios", "dev-1", tx1, false, false);
        let id = reg
            .register("j", "phone", "ios", "dev-1", tx2, false, false)
            .0;
        reg.register("someone", "laptop", "macos", "dev-2", tx3, false, false);

        assert_eq!(reg.list(Some("j")).len(), 1);
        assert_eq!(reg.list(None).len(), 2);

        let sent = reg.send(Some("j"), None, LinkCommand::Pause).unwrap();
        assert_eq!(sent.id, id);
        assert_eq!(rx2.try_recv().unwrap().command, LinkCommand::Pause);

        // Another account's device is not this account's to command.
        assert!(
            reg.send(Some("j"), Some("laptop"), LinkCommand::Pause)
                .is_err()
        );

        reg.unregister(&id);
        assert!(reg.send(Some("j"), None, LinkCommand::Pause).is_err());
    }

    #[test]
    fn disconnecting_an_account_closes_only_its_links() {
        let reg = Registry::default();
        let (tx1, mut rx1) = super::LinkTx::channel();
        let (tx2, mut rx2) = super::LinkTx::channel();
        reg.register("j", "phone", "ios", "dev-1", tx1, false, false);
        reg.register("someone", "laptop", "macos", "dev-2", tx2, false, false);

        reg.disconnect("j");
        assert!(reg.list(Some("j")).is_empty());
        // The session sees its channel close, which is what ends it.
        assert!(matches!(
            rx1.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected)
        ));
        assert_eq!(reg.list(Some("someone")).len(), 1);
        assert!(matches!(
            rx2.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
    }

    /// `j` shares the phone with `k`, not the Mac.
    fn shared() -> (
        Registry,
        tokio::sync::mpsc::Receiver<Envelope>,
        tokio::sync::mpsc::Receiver<Envelope>,
        tokio::sync::mpsc::Receiver<Envelope>,
    ) {
        let reg = Registry::default();
        let (phone_tx, mut phone) = super::LinkTx::channel();
        let (mac_tx, mac) = super::LinkTx::channel();
        let (k_tx, k) = super::LinkTx::channel();
        let id = reg
            .register("j", "phone", "ios", "dev-phone", phone_tx, true, false)
            .0;
        reg.register("j", "mac", "macos", "dev-mac", mac_tx, true, false);
        reg.register("k", "laptop", "macos", "dev-k", k_tx, true, false);
        reg.report(
            &id,
            LinkState {
                playing: true,
                title: Some("Roygbiv".into()),
                outputs: Some(Default::default()),
                ..Default::default()
            },
        );
        reg.share("j", "dev-phone", "k", true).unwrap();
        drain(&mut phone);
        (reg, phone, mac, k)
    }

    #[test]
    fn a_grantee_sees_the_shared_device_and_nothing_else_of_the_owner() {
        let (_reg, _phone, _mac, mut k) = shared();
        let listed = drain(&mut k)
            .into_iter()
            .rev()
            .find_map(|c| match c {
                LinkCommand::Devices { devices } => Some(devices),
                _ => None,
            })
            .expect("told of it");
        assert_eq!(listed.len(), 1, "the phone, not the Mac");
        let phone = &listed[0];
        assert_eq!(phone.id, "dev-phone");
        assert_eq!(phone.owner.as_deref(), Some("j"));
        let state = phone.state.as_ref().expect("its state");
        assert_eq!(state.title.as_deref(), Some("Roygbiv"));
        assert!(
            state.outputs.is_some(),
            "outputs, to choose from: the output is in the playback set"
        );
    }

    /// A granted account (`k`, say read-only) controlling the owner's (`j`,
    /// say admin) phone gets the playback set and nothing of `j`'s account:
    /// what touches the library is refused, and every command arrives marked
    /// as `k`'s, so the phone runs it with no power to sync, and no command
    /// in the set writes favourites, playlists or history.
    #[test]
    fn a_grantee_controls_playback_as_itself_and_nothing_of_the_owners_account() {
        let (reg, mut phone, mut mac, _k) = shared();
        let wrapped = |c: LinkCommand| LinkCommand::Shared {
            command: Box::new(c),
        };
        for cmd in [
            LinkCommand::Pause,
            LinkCommand::SetRendererVolume { volume: 40 },
            LinkCommand::SleepTimer {
                timer: Some(koan_core::player::state::SleepTimer::After { minutes: 30 }),
            },
            LinkCommand::HandOff { to: "dev-k".into() },
        ] {
            reg.relay("k", "dev-phone", cmd.clone()).unwrap();
            assert_eq!(drain(&mut phone), [wrapped(cmd)]);
        }
        for cmd in [
            LinkCommand::Sync { full: false },
            LinkCommand::Evict { track_ids: vec![] },
            LinkCommand::Shared {
                command: Box::new(LinkCommand::Pause),
            },
        ] {
            assert!(!cmd.allowed_playback());
            assert!(reg.relay("k", "dev-phone", cmd).is_err());
        }
        assert!(drain(&mut phone).is_empty());
        assert!(
            reg.relay("k", "dev-mac", LinkCommand::Pause).is_err(),
            "not shared, not reachable"
        );
        assert!(
            drain(&mut mac)
                .iter()
                .all(|c| matches!(c, LinkCommand::Devices { .. } | LinkCommand::Shares { .. })),
            "news, and no command"
        );
        assert!(reg.shared_owner("k", "dev-mac").is_none(), "nor wakeable");
    }

    /// "Move here" both ways: the grantee pulls the shared phone's music to
    /// its own device, which the phone sends as a hand-off; the grant lets
    /// that one command run backwards and nothing else.
    #[test]
    fn a_hand_off_runs_both_ways_across_a_grant_and_nothing_else_does() {
        let (reg, _phone, _mac, mut k) = shared();
        drain(&mut k);
        let play = LinkCommand::Play {
            track_ids: vec!["t".into()],
            start_at: 0,
            position_ms: 1000,
            paused: false,
            handoff: true,
            mode: koan_core::player::state::QueueMode::Keep,
        };
        reg.relay_from("j", Some("dev-phone"), "dev-k", play.clone())
            .unwrap();
        assert!(drain(&mut k).contains(&LinkCommand::Shared {
            command: Box::new(play.clone())
        }));
        assert!(
            reg.relay_from("j", Some("dev-phone"), "dev-k", LinkCommand::Pause)
                .is_err(),
            "the owner's phone does not command the grantee's devices"
        );
        assert!(
            reg.relay_from("j", Some("dev-mac"), "dev-k", play.clone())
                .is_err(),
            "only the shared device"
        );
        let not_a_hand_off = LinkCommand::Play {
            track_ids: vec!["t".into()],
            start_at: 0,
            position_ms: 0,
            paused: false,
            handoff: false,
            mode: koan_core::player::state::QueueMode::Keep,
        };
        assert!(
            reg.relay_from("j", Some("dev-phone"), "dev-k", not_a_hand_off)
                .is_err(),
            "a hand-off, not any play"
        );
    }

    #[test]
    fn revoking_ends_control_at_once() {
        let (reg, mut phone, _mac, mut k) = shared();
        reg.share("j", "dev-phone", "k", false).unwrap();
        assert!(reg.relay("k", "dev-phone", LinkCommand::Pause).is_err());
        assert!(
            drain(&mut phone)
                .iter()
                .all(|c| !matches!(c, LinkCommand::Pause))
        );
        assert!(reg.shared_owner("k", "dev-phone").is_none());
        let listed = drain(&mut k)
            .into_iter()
            .rev()
            .find_map(|c| match c {
                LinkCommand::Devices { devices } => Some(devices),
                _ => None,
            })
            .expect("told it is gone");
        assert!(listed.is_empty());
    }

    #[test]
    fn a_device_hears_whom_it_is_shared_with_every_time_it_links() {
        let reg = Registry::default();
        let (tx, mut phone) = super::LinkTx::channel();
        reg.register("j", "phone", "ios", "dev-phone", tx, true, false);
        let first = drain(&mut phone);
        assert!(
            first.contains(&LinkCommand::Shares {
                grantees: vec![],
                error: None,
                accounts: vec![],
            }),
            "an empty list replaces one from another server"
        );
        reg.send_shares(
            "j",
            "dev-phone",
            Some("There is no account called x".into()),
        );
        assert!(drain(&mut phone).iter().any(
            |c| matches!(c, LinkCommand::Shares { error: Some(e), .. } if e.contains("no account"))
        ));
    }

    #[test]
    fn the_owner_is_told_who_it_shares_with() {
        let reg = Registry::default();
        let (tx, mut phone) = super::LinkTx::channel();
        reg.register("j", "phone", "ios", "dev-phone", tx, true, false);
        reg.share("j", "dev-phone", "k", true).unwrap();
        reg.share("j", "dev-phone", "m", true).unwrap();
        let last = drain(&mut phone).into_iter().rev().find_map(|c| match c {
            LinkCommand::Shares { grantees, .. } => Some(grantees),
            _ => None,
        });
        assert_eq!(last, Some(vec!["k".to_string(), "m".to_string()]));
        assert!(
            reg.share("j", "dev-phone", "j", true).is_err(),
            "not with itself"
        );
    }

    /// Whose token wakes a device: another account's, behind the same router
    /// as the asker; a shared one's owner from anywhere; and otherwise only
    /// the asker's own, so a device elsewhere of another account is refused.
    #[test]
    fn a_device_is_woken_for_another_account_on_its_network_or_by_grant() {
        let reg = Registry::default();
        let home: std::net::IpAddr = "203.0.113.7".parse().unwrap();
        let away: std::net::IpAddr = "198.51.100.2".parse().unwrap();
        reg.seen_at("dev-ipad", "sarita", home);
        reg.seen_at("dev-phone", "admin", home);
        assert_eq!(
            reg.wake_owner("admin", "dev-phone", "dev-ipad"),
            "sarita",
            "same address: the iPad's own token"
        );
        reg.seen_at("dev-phone", "admin", away);
        assert_eq!(
            reg.wake_owner("admin", "dev-phone", "dev-ipad"),
            "admin",
            "elsewhere, with no grant: only admin's own, which it is not"
        );
        reg.share("sarita", "dev-ipad", "admin", true).unwrap();
        assert_eq!(
            reg.wake_owner("admin", "dev-phone", "dev-ipad"),
            "sarita",
            "shared: from anywhere"
        );
        // An address is believed only for the account that linked from it.
        reg.seen_at("dev-other", "mallory", home);
        assert_eq!(reg.wake_owner("admin", "dev-other", "dev-tv"), "admin");
    }

    /// The server restarts with every release; a backgrounded iPad has to
    /// stay wakeable across one without being opened again.
    #[test]
    fn where_a_device_last_linked_from_survives_a_restart() {
        use koan_core::db::queries::link;
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        koan_core::db::schema::create_tables(&conn).unwrap();
        let home: std::net::IpAddr = "203.0.113.7".parse().unwrap();
        for (device, user, seen) in [("dev-ipad", "sarita", 1), ("dev-phone", "admin", 2)] {
            link::record_device(&conn, device, user, device, "ios", seen).unwrap();
            link::save_address(&conn, device, user, home).unwrap();
        }

        // A fresh server, from what was saved.
        let reg = Registry::default();
        *reg.addresses.lock() = link::addresses(&conn).unwrap();
        assert_eq!(
            reg.wake_owner("admin", "dev-phone", "dev-ipad"),
            "sarita",
            "still woken through its own account after the restart"
        );
    }

    /// Forgetting a device tells the account's linked devices to drop it,
    /// and no other account's; a linked device is not forgotten, since it
    /// would be back at once.
    #[test]
    fn a_forgotten_device_is_dropped_by_the_accounts_devices() {
        let reg = Registry::default();
        let (mac_tx, mut mac) = super::LinkTx::channel();
        let (other_tx, mut other) = super::LinkTx::channel();
        reg.register("j", "mac", "macos", "dev-mac", mac_tx, true, false);
        reg.register("k", "laptop", "macos", "dev-k", other_tx, true, false);
        while mac.try_recv().is_ok() {}
        while other.try_recv().is_ok() {}

        reg.forget("j", "dev-phone").unwrap();
        let heard: Vec<LinkCommand> =
            std::iter::from_fn(|| mac.try_recv().ok().map(|e| e.command)).collect();
        assert!(heard.contains(&LinkCommand::Forgotten {
            device: "dev-phone".into()
        }));
        assert!(
            std::iter::from_fn(|| other.try_recv().ok().map(|e| e.command))
                .all(|c| !matches!(c, LinkCommand::Forgotten { .. })),
            "another account hears nothing of it"
        );
        assert!(
            reg.forget("j", "dev-mac").is_err(),
            "linked: it would be back"
        );
        assert!(
            reg.relay(
                "j",
                "dev-mac",
                LinkCommand::Forgotten { device: "x".into() }
            )
            .is_err(),
            "news from the server, not a command a device may send"
        );
    }

    /// A device shared by another account is forgotten by declining the
    /// share: its owner keeps it and is told, and it is not listed again.
    #[test]
    fn forgetting_a_shared_device_declines_the_share() {
        let (reg, mut phone, _mac, mut k) = shared();
        drain(&mut k);
        reg.forget("k", "dev-phone").unwrap();
        assert!(reg.shared_owner("k", "dev-phone").is_none());
        assert!(
            drain(&mut phone)
                .iter()
                .any(|c| matches!(c, LinkCommand::Shares { grantees, .. } if grantees.is_empty()))
        );
        let listed = drain(&mut k).into_iter().rev().find_map(|c| match c {
            LinkCommand::Devices { devices } => Some(devices),
            _ => None,
        });
        assert_eq!(listed.map(|d| d.len()), Some(0));
    }
}
