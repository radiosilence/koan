//! Getting a command to a device: down its link, answered or given up on,
//! queued for one that is away and woken by push, or as a notification to tap
//! for music an asleep iPhone cannot start by itself.

use super::*;

impl Registry {
    /// Wake `to`, one of `username`'s devices that is not linked, for `from`
    /// (a device id), which chose it: with a background push, or with
    /// `notify` a notification asking to be tapped. Logged with the push's
    /// round trip, and remembered so the link that follows is logged with how
    /// long it took.
    pub fn wake(&self, username: &str, from: &str, to: &str, notify: bool) {
        let from_name = self
            .list(Some(username))
            .into_iter()
            .find(|c| c.device == from)
            .map_or_else(|| "Another device".to_string(), |c| c.name);
        let owner = self.wake_owner(username, from, to);
        if owner != username {
            log::info!("wake: {to} is {owner}'s, woken for {username}");
        }
        let username = &owner;
        if self.list(Some(username)).iter().any(|c| c.device == to) {
            log::info!("wake: {to} is already linked");
            return;
        }
        let Some(pusher) = crate::push::pusher() else {
            log::info!("wake: push is off, so {to} cannot be woken");
            return;
        };
        let Some(target) = outbox::push_targets(Some(username))
            .into_iter()
            .find(|t| t.device == to)
        else {
            log::info!("wake: {to} has given no push handle");
            return;
        };
        let (push, how) = if notify {
            log::info!("wake: {from_name} asks {to} to let it play");
            (crate::push::Push::Summon, "notification")
        } else {
            (crate::push::Push::WakeNow, "wake push")
        };
        {
            let mut woken = WOKEN.lock();
            // A wake not answered within the hour never will be.
            woken.retain(|_, (sent, _)| sent.elapsed() < WOKEN_FOR);
            woken.insert(
                (target.device.clone(), target.username.clone()),
                (std::time::Instant::now(), how),
            );
        }
        std::thread::spawn(move || {
            let started = std::time::Instant::now();
            deliver_push(pusher, &target, &push);
            log::info!(
                "wake: {how} for {} answered by the relay in {}ms",
                target.name,
                started.elapsed().as_millis()
            );
        });
    }
}

impl Registry {
    /// Relay `command` from one of `username`'s devices to another, `to`.
    pub fn relay(
        &self,
        username: &str,
        to: &str,
        command: LinkCommand,
    ) -> Result<ClientInfo, String> {
        self.relay_from(username, None, to, command)
    }

    /// `relay`, knowing which of `username`'s devices, `from`, sent it: what
    /// lets a device shared with another account hand its music to that
    /// account's device, the one way a grant runs backwards.
    pub fn relay_from(
        &self,
        username: &str,
        from: Option<&str>,
        to: &str,
        command: LinkCommand,
    ) -> Result<ClientInfo, String> {
        self.relay_acked(username, from, to, command, None)
    }

    /// `relay_from`, with the device's answer handed to `acking`'s reply.
    pub fn relay_acked(
        &self,
        username: &str,
        from: Option<&str>,
        to: &str,
        command: LinkCommand,
        acking: Option<Acking>,
    ) -> Result<ClientInfo, String> {
        // What one device may have another do, and nothing the server says
        // itself: relayed, a forged device list or key list would reach the
        // device as the server's own.
        if !command.relayable() {
            return Err("not a command".into());
        }
        // A device shared with `username` takes the playback set, marked as
        // another account's so it runs as that account's request and never
        // with its owner's powers.
        if let Some(owner) = self.shared_owner(username, to) {
            if !command.allowed_playback() {
                return Err(format!("{to} is shared for playback only"));
            }
            let command = LinkCommand::Shared {
                command: Box::new(command),
            };
            return self.send_with(Some(&owner), Some(to), command, acking);
        }
        // The other way: a device shared with `to`'s account hands its music
        // there, as "Move here" from that account asks it to. That and only
        // that: a grant does not let the owner's device command the grantee's.
        if let Some(from) = from
            && let Some(grantee) = self.grantee_of(username, from, to)
        {
            if !matches!(command, LinkCommand::Play { handoff: true, .. }) {
                return Err(format!("{to} is another account's"));
            }
            let command = LinkCommand::Shared {
                command: Box::new(command),
            };
            return self.send_with(Some(&grantee), Some(to), command, acking);
        }
        self.send_with(Some(username), Some(to), command, acking)
    }
}

impl Registry {
    /// Send to `id` (an id or a name), or with none to the client the
    /// command most likely means: the one playing, else the one that played
    /// within `RECENT`, else the only one linked. `Err` names the choices when
    /// there is no telling, so whoever asked can ask the person.
    pub fn send(
        &self,
        username: Option<&str>,
        id: Option<&str>,
        cmd: LinkCommand,
    ) -> Result<ClientInfo, String> {
        self.send_with(username, id, cmd, None)
    }

    /// `send`, and with `acking` the device's answer handed to its `reply`.
    /// A device that is away is answered for: `Queued`, once it has been woken
    /// to take the command. One whose link has gone quiet since the command
    /// was sent is taken for away, and woken; one that is only slow is given
    /// longer. One that gives no answers is answered `None` at once.
    pub fn send_with(
        &self,
        username: Option<&str>,
        id: Option<&str>,
        cmd: LinkCommand,
        acking: Option<Acking>,
    ) -> Result<ClientInfo, String> {
        let clients = self.list(username);
        let target = match id {
            Some(id) => clients
                .iter()
                .find(|c| c.id == id || c.device == id || c.name.eq_ignore_ascii_case(id)),
            None if clients.is_empty() => None,
            None => Some(pick(&clients, chrono::Utc::now().timestamp())?),
        };
        // Not linked: a phone iOS has suspended is woken to take it.
        let Some(target) = target else {
            let reached = reach_absent(username, id, &cmd, acking.as_ref().map(|a| a.id))
                .unwrap_or_else(|| {
                    Err(match id {
                        Some(id) => format!("no linked client {id}; see `clients`"),
                        None => {
                            "no koan app is linked to this server; open koan on the device".into()
                        }
                    })
                });
            if let (Ok(_), Some(acking)) = (&reached, acking) {
                (acking.reply)(Some(AckOutcome::Queued));
            }
            return reached;
        };
        let target = target.clone();
        let entries = self.entries.lock();
        let entry = entries
            .iter()
            .find(|e| e.info.id == target.id)
            .ok_or("that client has just gone")?;
        let (account, device) = (entry.info.username.clone(), entry.device.clone());
        // Listened for before it is sent, so a quick answer is not missed.
        // Nothing is answered while the entries are held: an answer may relay
        // to another link.
        let (ack, unanswered) = match acking {
            Some(acking) if entry.info.acks => {
                self.answers.lock().insert(
                    (account.clone(), device.clone(), acking.id),
                    Waiting {
                        reply: acking.reply,
                        received: false,
                        timer: None,
                    },
                );
                (Some(acking.id), None)
            }
            other => (None, other),
        };
        let sent = entry.tx.send(Envelope {
            command: cmd.clone(),
            ack,
        });
        drop(entries);
        if sent.is_err() {
            if let Some(w) = ack.and_then(|ack| self.answers.lock().remove(&(account, device, ack)))
            {
                w.answer(Some(AckOutcome::Failed {
                    error: "its link has just gone".into(),
                }));
            }
            return Err("that client has just gone".to_string());
        }
        if let Some(acking) = unanswered {
            (acking.reply)(None);
        }
        if let Some(ack) = ack {
            self.watch(account, device, ack, cmd);
        }
        Ok(target)
    }

    /// Look at the command sent to `device` under `ack` after `FIRST_LOOK`,
    /// and give up on its answer after `LONGEST_ANSWER`: a timer on the
    /// server's runtime, kept with the waiting answer and stopped when the
    /// answer comes. Outside a runtime (a test) nothing looks.
    pub(super) fn watch(&self, username: String, device: String, ack: u64, cmd: LinkCommand) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let key = (username.clone(), device.clone(), ack);
        let timer = runtime.spawn(async move {
            tokio::time::sleep(FIRST_LOOK).await;
            let (looking, looking_in) = (device.clone(), username.clone());
            let taken = tokio::task::spawn_blocking(move || {
                registry().look(&looking_in, &looking, ack, &cmd)
            })
            .await
            .unwrap_or(false);
            if taken {
                tokio::time::sleep(LONGEST_ANSWER.saturating_sub(FIRST_LOOK)).await;
                registry().give_up(&username, &device, ack);
            }
        });
        match self.answers.lock().get_mut(&key) {
            Some(w) => w.timer = Some(timer.abort_handle()),
            // Answered already.
            None => timer.abort(),
        }
    }

    /// `device`, linked as `username`, answered the command it was sent
    /// under `ack`.
    pub fn answered(&self, username: &str, device: &str, ack: u64, outcome: AckOutcome) {
        let key = (username.to_string(), device.to_string(), ack);
        let waiting = self.answers.lock().remove(&key);
        if let Some(w) = waiting {
            w.answer(Some(outcome));
        }
    }

    /// `device`, linked as `username`, says the command sent under `ack` came
    /// off its link.
    pub fn received(&self, username: &str, device: &str, ack: u64) {
        let key = (username.to_string(), device.to_string(), ack);
        if let Some(w) = self.answers.lock().get_mut(&key) {
            w.received = true;
        }
    }

    /// The first look at a command sent to `device` under `ack`. Taken off its
    /// link: its answer is waited for, however long the command runs, and
    /// `true` says so. Not taken: the link is not delivering, though it may be
    /// open (a phone iOS has suspended, a socket not yet closed), so the
    /// device is reached as one that is away, with the command kept under its
    /// id so the device acts on it once whichever copy arrives first. The link
    /// is left to end of its own accord.
    pub(super) fn look(&self, username: &str, device: &str, ack: u64, cmd: &LinkCommand) -> bool {
        let key = (username.to_string(), device.to_string(), ack);
        let mut answers = self.answers.lock();
        match answers.get(&key) {
            None => return false,
            Some(w) if w.received => return true,
            Some(_) => {}
        }
        let w = answers.remove(&key).expect("just seen");
        drop(answers);
        log::info!("link: {device} has not taken a command; reaching it as away");
        let outcome = match reach_absent(Some(username), Some(device), cmd, Some(ack)) {
            Some(Ok(_)) => AckOutcome::Queued,
            _ => AckOutcome::Failed {
                error: format!("{device} did not take it"),
            },
        };
        w.answer(Some(outcome));
        false
    }

    /// Stop waiting for the answer to a command `device` took long ago; the
    /// asker learns only that it arrived.
    pub(super) fn give_up(&self, username: &str, device: &str, ack: u64) {
        let key = (username.to_string(), device.to_string(), ack);
        let waiting = self.answers.lock().remove(&key);
        if let Some(w) = waiting {
            w.answer(None);
        }
    }
}

impl Registry {
    /// Send to every client `username` may command. The names of those it
    /// reached.
    pub fn broadcast(&self, username: Option<&str>, cmd: LinkCommand) -> Vec<String> {
        let ids: Vec<String> = self.list(username).into_iter().map(|c| c.id).collect();
        let entries = self.entries.lock();
        entries
            .iter()
            .filter(|e| ids.contains(&e.info.id) && e.tx.send(cmd.clone()).is_ok())
            .map(|e| e.info.name.clone())
            .collect()
    }
}

impl Registry {
    /// Send to every device `username` may command: at once to those linked,
    /// and to those that have linked before but are away now, when they next
    /// link. For commands still right hours later (`Sync`, `Evict`), never
    /// playback. The names reached now, and the names it waits for.
    pub fn deliver(&self, username: Option<&str>, cmd: LinkCommand) -> (Vec<String>, Vec<String>) {
        let (sent, queued) = self.link_or_queue(username, &cmd);
        wake(&queued.iter().map(Absent::key).collect::<Vec<_>>());
        (sent, queued.into_iter().map(|q| q.name).collect())
    }

    pub(super) fn link_or_queue(
        &self,
        username: Option<&str>,
        cmd: &LinkCommand,
    ) -> (Vec<String>, Vec<Absent>) {
        let sent = self.broadcast(username, cmd.clone());
        let queued = outbox::queue_for_absent(username, &self.live(), cmd);
        (sent, queued)
    }

    /// Each linked device, as `(device, username)`.
    pub(super) fn live(&self) -> Vec<(String, String)> {
        self.entries
            .lock()
            .iter()
            .map(|e| (e.device.clone(), e.info.username.clone()))
            .collect()
    }
}

/// Wake these absent devices, each a `(device, username)`, where a push can:
/// each links, and takes what was queued for it.
pub(super) fn wake(devices: &[(String, String)]) {
    let Some(pusher) = crate::push::pusher() else {
        return;
    };
    let targets: Vec<outbox::PushTarget> = outbox::push_targets(None)
        .into_iter()
        .filter(|t| {
            devices
                .iter()
                .any(|(device, username)| *device == t.device && *username == t.username)
        })
        .collect();
    if targets.is_empty() {
        return;
    }
    std::thread::spawn(move || {
        for t in targets {
            deliver_push(pusher, &t, &crate::push::Push::Wake);
        }
    });
}

/// Reach a device that is not linked: the one named, else the one seen most
/// recently.
///
/// Music is sent as a notification to tap, at once: iOS does not let an app it
/// woke start audio, so waking it for that only delays the notification. Every
/// other command goes into the device's outbox with a background push to wake
/// it, and runs as if it had been linked. `None` without a push key, or with no
/// device in scope that has given a push token.
pub(super) fn reach_absent(
    username: Option<&str>,
    id: Option<&str>,
    cmd: &LinkCommand,
    ack: Option<u64>,
) -> Option<Result<ClientInfo, String>> {
    if cmd.live_only() {
        return None;
    }
    let envelope = Envelope {
        command: cmd.clone(),
        ack,
    };
    let pusher = crate::push::pusher()?;
    // Most recently seen first: a reinstall leaves its old entry behind under
    // the same name, and the newest is the one in the person's hand.
    let target = outbox::push_targets(username)
        .into_iter()
        .find(|t| id.is_none_or(|id| t.device == id || t.name.eq_ignore_ascii_case(id)))?;
    let info = ClientInfo {
        id: target.device.clone(),
        device: target.device.clone(),
        name: target.name.clone(),
        platform: target.platform.clone(),
        username: target.username.clone(),
        connected_at: 0,
        state: LinkState::default(),
        last_played_at: None,
        state_at: 0,
        reports: false,
        notified: true,
        acks: false,
    };
    // What it asks, whoever asks it.
    let asked = match cmd {
        LinkCommand::Shared { command } => command.as_ref(),
        cmd => cmd,
    };
    let verb = match asked {
        LinkCommand::Play { .. } | LinkCommand::JumpTo { .. } | LinkCommand::Resume => Some("Play"),
        _ => None,
    };
    let push = match verb {
        Some(verb) => {
            log::info!(
                "push: {verb} on {}: {}",
                target.name,
                outbox::describe(asked).unwrap_or_default()
            );
            crate::push::Push::Notify {
                claim: claims::keep(&target.device, &target.username, envelope),
            }
        }
        None => {
            outbox::queue_for(&target.device, &target.username, &envelope);
            crate::push::Push::Wake
        }
    };
    std::thread::spawn(move || deliver_push(pusher, &target, &push));
    Some(Ok(info))
}

/// Commands a notification asked the person to tap for, kept for the app to
/// claim over the link once they do. The notification carries only the
/// claim's id, which tells the relay, and anyone else, nothing. In memory: a
/// notification lives ten minutes, and a claim no longer than that.
pub mod claims {
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    use koan_core::remote::acks::Envelope;
    use parking_lot::Mutex;

    /// As long as the relay keeps a notification: one tapped later than this
    /// has nothing to claim.
    const LIFE: Duration = Duration::from_secs(10 * 60);

    struct Claim {
        device: String,
        username: String,
        envelope: Envelope,
        at: Instant,
    }

    static CLAIMS: Mutex<Option<HashMap<String, Claim>>> = Mutex::new(None);

    /// Keep `envelope` for `device`, and the id it is claimed by.
    pub fn keep(device: &str, username: &str, envelope: Envelope) -> String {
        use base64::Engine;
        let mut raw = [0u8; 16];
        getrandom::fill(&mut raw).expect("system randomness");
        let id = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw);
        let mut claims = CLAIMS.lock();
        let claims = claims.get_or_insert_with(HashMap::new);
        claims.retain(|_, c| c.at.elapsed() < LIFE);
        claims.insert(
            id.clone(),
            Claim {
                device: device.to_string(),
                username: username.to_string(),
                envelope,
                at: Instant::now(),
            },
        );
        id
    }

    /// The command kept under `id`, once, for the device it was kept for.
    pub fn take(device: &str, username: &str, id: &str) -> Option<Envelope> {
        let mut claims = CLAIMS.lock();
        let claims = claims.as_mut()?;
        let claim = claims.get(id)?;
        if claim.device != device || claim.username != username || claim.at.elapsed() >= LIFE {
            return None;
        }
        claims.remove(id).map(|c| c.envelope)
    }
}

/// Send one push, forgetting a handle the relay says no longer reaches the
/// device.
pub(super) fn deliver_push(
    pusher: &crate::push::Pusher,
    target: &outbox::PushTarget,
    push: &crate::push::Push,
) {
    use crate::push::Outcome;
    match pusher.send(&target.handle, push) {
        Outcome::Sent => log::info!("push: sent to {}", target.name),
        Outcome::Gone => {
            log::info!(
                "push: {}'s handle no longer reaches it; forgotten",
                target.name
            );
            outbox::forget_push(&target.username, &target.device);
        }
        Outcome::Failed(e) => log::warn!("push: to {} failed: {e}", target.name),
    }
}

/// Wakes sent and not yet answered by a link, by `(device, username)`: when,
/// and which kind. A device that does not link is never answered, so each
/// entry is dropped after `WOKEN_FOR`.
pub(super) const WOKEN_FOR: std::time::Duration = std::time::Duration::from_secs(60 * 60);

pub(super) type Woken =
    std::collections::HashMap<(String, String), (std::time::Instant, &'static str)>;

pub(super) static WOKEN: LazyLock<Mutex<Woken>> = LazyLock::new(Default::default);

/// How long ago a client can have stopped playing and still be the obvious
/// one to send music to.
pub(super) const RECENT: i64 = 6 * 60 * 60;

pub(super) fn pick(clients: &[ClientInfo], now: i64) -> Result<&ClientInfo, String> {
    if let Some(c) = clients.iter().find(|c| c.state.playing) {
        return Ok(c);
    }
    if let Some(c) = clients
        .iter()
        .filter(|c| c.last_played_at.is_some_and(|t| now - t < RECENT))
        .max_by_key(|c| c.last_played_at)
    {
        return Ok(c);
    }
    match clients {
        [] => Err("no koan app is linked to this server; open koan on the device".into()),
        [only] => Ok(only),
        several => Err(format!(
            "several koan apps are linked and none has played recently: {}. Ask which, then pass `client`",
            several
                .iter()
                .map(|c| format!("{} ({}, id {})", c.name, c.platform, c.device))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clients::testing::*;

    /// A link that has said nothing for a while is not dead for it: a device
    /// that took the command keeps its link and is waited for, however long
    /// the command runs. One that has not taken it is reached as away, and
    /// its link is still left alone.
    #[test]
    fn a_quiet_link_that_took_the_command_is_kept_and_waited_for() {
        use koan_core::remote::acks::AckOutcome;
        let reg = Registry::default();
        let (tx, _rx) = super::LinkTx::channel();
        reg.register("q", "mac", "macos", "dev-quiet", tx, false, true);
        let answers = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let acking = |id: u64| {
            let answers = answers.clone();
            Acking {
                id,
                reply: Box::new(move |outcome| answers.lock().unwrap().push((id, outcome))),
            }
        };

        // Taken off its link, then busy (a sync) and silent: kept, waited for.
        reg.relay_acked("q", None, "dev-quiet", LinkCommand::Pause, Some(acking(1)))
            .unwrap();
        reg.received("q", "dev-quiet", 1);
        assert!(reg.look("q", "dev-quiet", 1, &LinkCommand::Pause));
        assert!(answers.lock().unwrap().is_empty(), "still waiting");
        reg.answered("q", "dev-quiet", 1, AckOutcome::Done);
        assert_eq!(
            answers.lock().unwrap().last(),
            Some(&(1, Some(AckOutcome::Done)))
        );

        // Not taken: reached as away (no push key here, so not reachable),
        // and the link itself left alone.
        reg.relay_acked("q", None, "dev-quiet", LinkCommand::Pause, Some(acking(2)))
            .unwrap();
        assert!(!reg.look("q", "dev-quiet", 2, &LinkCommand::Pause));
        assert!(matches!(
            answers.lock().unwrap().last(),
            Some((2, Some(AckOutcome::Failed { .. })))
        ));
        assert_eq!(reg.list(Some("q")).len(), 1, "the link is not dropped");
    }

    /// An answered command costs nothing more: the answer takes its waiting
    /// entry, and that stops the timer that would have looked at it.
    #[tokio::test]
    async fn an_answer_stops_its_timer() {
        let timer = tokio::spawn(tokio::time::sleep(std::time::Duration::from_secs(3600)));
        let handle = timer.abort_handle();
        let waiting = Waiting {
            reply: Box::new(|_| {}),
            received: true,
            timer: Some(timer.abort_handle()),
        };
        waiting.answer(Some(koan_core::remote::acks::AckOutcome::Done));
        assert!(timer.await.unwrap_err().is_cancelled());
        assert!(handle.is_finished());
    }

    #[test]
    fn a_command_relayed_with_an_id_is_answered_once() {
        use koan_core::remote::acks::AckOutcome;
        let reg = Registry::default();
        let (tx, mut rx) = super::LinkTx::channel();
        reg.register("a", "phone", "ios", "dev-answers", tx, false, true);
        let (old_tx, mut old_rx) = super::LinkTx::channel();
        reg.register("a", "mac", "macos", "dev-old", old_tx, false, false);
        let answers = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let acking = |id: u64| {
            let answers = answers.clone();
            Acking {
                id,
                reply: Box::new(move |outcome| answers.lock().unwrap().push((id, outcome))),
            }
        };

        // A device that answers is sent the id, and its answer handed on once.
        reg.relay_acked(
            "a",
            None,
            "dev-answers",
            LinkCommand::Pause,
            Some(acking(7)),
        )
        .unwrap();
        assert_eq!(rx.try_recv().unwrap().ack, Some(7));
        // Another account's device claiming the same id is not heard.
        let (b_tx, _b_rx) = super::LinkTx::channel();
        reg.register("b", "phone", "ios", "dev-answers", b_tx, false, true);
        reg.received("b", "dev-answers", 7);
        reg.answered("b", "dev-answers", 7, AckOutcome::Done);
        assert!(
            answers.lock().unwrap().is_empty(),
            "b cannot answer a's command"
        );
        reg.answered("a", "dev-answers", 7, AckOutcome::Done);
        reg.answered("a", "dev-answers", 7, AckOutcome::Done);

        // One that predates answers gets the command plain, and the asker
        // learns at once that no answer is coming.
        reg.relay_acked("a", None, "dev-old", LinkCommand::Pause, Some(acking(8)))
            .unwrap();
        assert_eq!(old_rx.try_recv().unwrap().ack, None);

        assert_eq!(
            *answers.lock().unwrap(),
            [(7, Some(AckOutcome::Done)), (8, None)]
        );
    }

    #[test]
    fn a_claim_runs_once_and_only_on_its_device() {
        let envelope = Envelope {
            command: LinkCommand::Pause,
            ack: None,
        };
        let id = claims::keep("phone", "j", envelope);
        assert!(claims::take("tablet", "j", &id).is_none(), "another device");
        assert!(claims::take("phone", "k", &id).is_none(), "another account");
        assert_eq!(
            claims::take("phone", "j", &id).map(|e| e.command),
            Some(LinkCommand::Pause)
        );
        assert!(claims::take("phone", "j", &id).is_none(), "taken once");
        assert!(claims::take("phone", "j", "made-up").is_none());
    }

    #[test]
    fn the_device_playing_is_the_one_meant() {
        let reg = Registry::default();
        let (tx1, _rx1) = super::LinkTx::channel();
        let (tx2, mut rx2) = super::LinkTx::channel();
        let mac = reg
            .register("j", "mac", "macos", "dev-1", tx1, false, false)
            .0;
        let phone = reg
            .register("j", "phone", "ios", "dev-2", tx2, false, false)
            .0;

        // Two idle devices: no telling, so the caller is told to ask.
        let err = reg.send(Some("j"), None, LinkCommand::Pause).unwrap_err();
        assert!(err.contains("mac") && err.contains("phone"), "{err}");

        reg.report(
            &phone,
            LinkState {
                playing: true,
                ..Default::default()
            },
        );
        assert_eq!(
            reg.send(Some("j"), None, LinkCommand::Pause).unwrap().id,
            phone
        );
        assert_eq!(rx2.try_recv().unwrap().command, LinkCommand::Pause);

        // Stopped a moment ago: still the one meant, over the Mac.
        reg.report(&phone, LinkState::default());
        assert_eq!(
            reg.send(Some("j"), None, LinkCommand::Pause).unwrap().id,
            phone
        );
        let _ = mac;
    }

    #[test]
    fn a_device_hears_its_peers_and_commands_reach_them_by_device_id() {
        let reg = Registry::default();
        let (tx1, mut rx1) = super::LinkTx::channel();
        let (tx2, mut rx2) = super::LinkTx::channel();
        let (tx3, mut rx3) = super::LinkTx::channel();
        reg.register("j", "mac", "macos", "dev-mac", tx1, true, false);
        let phone = reg
            .register("j", "phone", "ios", "dev-phone", tx2, true, false)
            .0;
        reg.register("someone", "laptop", "macos", "dev-other", tx3, true, false);

        reg.report(
            &phone,
            LinkState {
                playing: true,
                title: Some("Roygbiv".into()),
                ..Default::default()
            },
        );
        let devices = drain(&mut rx1)
            .into_iter()
            .rev()
            .find_map(|c| match c {
                LinkCommand::Devices { devices } => Some(devices),
                _ => None,
            })
            .expect("the Mac is told of the phone");
        assert_eq!(devices.len(), 1, "not itself, not another account's");
        assert_eq!(devices[0].id, "dev-phone");
        assert_eq!(
            devices[0].state.as_ref().and_then(|s| s.title.as_deref()),
            Some("Roygbiv")
        );
        while rx3.try_recv().is_ok() {}
        assert!(rx3.try_recv().is_err());

        while rx2.try_recv().is_ok() {}
        reg.relay("j", "dev-phone", LinkCommand::Pause).unwrap();
        assert_eq!(rx2.try_recv().unwrap().command, LinkCommand::Pause);
        assert!(
            reg.relay("someone", "dev-phone", LinkCommand::Pause)
                .is_err(),
            "another account cannot reach it"
        );
        assert!(
            reg.relay("j", "dev-phone", LinkCommand::Devices { devices: vec![] })
                .is_err()
        );
    }
}
