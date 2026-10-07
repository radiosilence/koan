//! Commands a device answers once it has acted on them, so a sender knows a
//! command arrived rather than only that it was queued.
//!
//! A command that wants an answer carries an id beside it on the wire
//! (`Envelope`), never inside the `LinkCommand`: a server that predates this
//! parses and re-serialises relayed commands, and would drop it, while a
//! client that predates it reads the command and ignores the id. The target
//! answers with the id and an `AckOutcome`.
//!
//! A sender that hears nothing tries the next way through with the same id,
//! so a command can arrive twice. Each id is drawn at random, so none is
//! reused across a sender's restarts and none can be guessed from another
//! seen on the network, and a target remembers the ids it has taken for a
//! minute: a repeat is answered with the first one's outcome and not acted on
//! again. Only a command the target accepts from its source takes an id, so a
//! stranger's refused command cannot reserve one.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::remote::link::LinkCommand;

/// How a command went, as the device it was sent to says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "camelCase")]
pub enum AckOutcome {
    /// Acted on.
    Done,
    /// Not acted on yet, but kept for the device: it is asleep, and takes it
    /// when woken. Said by the server, for the device.
    Queued,
    /// Not allowed from where it came.
    Refused { reason: String },
    /// Taken, and it failed.
    Failed { error: String },
}

/// A command as it travels, with the id that asks for an answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    #[serde(flatten)]
    pub command: LinkCommand,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ack: Option<u64>,
}

impl Envelope {
    /// A command as a push carries it: the same JSON as over the link.
    pub fn parse(json: &str) -> Result<Self, String> {
        serde_json::from_str(json).map_err(|e| e.to_string())
    }
}

impl From<LinkCommand> for Envelope {
    fn from(command: LinkCommand) -> Self {
        Self { command, ack: None }
    }
}

/// How long a target remembers an id it has taken.
const REMEMBERED: Duration = Duration::from_secs(60);

// --- Sending ----------------------------------------------------------------

/// A fresh id for a command that wants an answer: random, and never 0.
pub fn next_id() -> u64 {
    let mut bytes = [0u8; 8];
    if getrandom::fill(&mut bytes).is_err() {
        // No randomness to be had: unguessable no longer, but still unique
        // within the run.
        static FALLBACK: AtomicU64 = AtomicU64::new(1);
        return FALLBACK.fetch_add(1, Ordering::Relaxed) | (1 << 63);
    }
    u64::from_le_bytes(bytes).max(1)
}

/// Each waiting id, with the device it was sent to and where its answer goes.
static WAITING: LazyLock<Mutex<HashMap<u64, (String, crossbeam_channel::Sender<AckOutcome>)>>> =
    LazyLock::new(Default::default);

/// Wait for `device`'s answer to the command sent under `id`. Dropping the
/// receiver stops waiting.
pub fn expect(id: u64, device: &str) -> crossbeam_channel::Receiver<AckOutcome> {
    let (tx, rx) = crossbeam_channel::bounded(1);
    WAITING.lock().insert(id, (device.to_owned(), tx));
    rx
}

/// `from` answered `id`, by whatever way. Only the device the command was
/// sent to can answer it: nearby connections are not encrypted, so an id is
/// seen by anyone on the network, and any device this one has dialled could
/// otherwise report a command done that never arrived, or refused one that
/// did.
pub fn resolve(id: u64, from: &str, outcome: AckOutcome) {
    let mut waiting = WAITING.lock();
    match waiting.get(&id) {
        Some((device, _)) if device == from => {}
        Some((device, _)) => {
            log::warn!("acks: {from} answered a command sent to {device}; ignored");
            return;
        }
        None => return,
    }
    if let Some((_, tx)) = waiting.remove(&id) {
        let _ = tx.try_send(outcome);
    }
}

/// No longer waiting for `id`.
pub fn forget(id: u64) {
    WAITING.lock().remove(&id);
}

// --- Receiving --------------------------------------------------------------

type Reply = Box<dyn FnOnce(AckOutcome) + Send>;

enum Taken {
    Running(Vec<Reply>),
    Answered(AckOutcome),
}

static TAKEN: LazyLock<Mutex<HashMap<u64, (Instant, Taken)>>> = LazyLock::new(Default::default);

/// A command taken under an id: finish it with its outcome, which answers the
/// sender and any repeat that arrived meanwhile. Dropped unfinished, it
/// answers that it failed.
pub struct Pending {
    id: u64,
    finished: bool,
}

impl Pending {
    pub fn finish(mut self, outcome: AckOutcome) {
        self.finished = true;
        answer(self.id, outcome);
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        if !self.finished {
            answer(
                self.id,
                AckOutcome::Failed {
                    error: "it stopped before finishing".into(),
                },
            );
        }
    }
}

impl std::fmt::Debug for Pending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Pending({})", self.id)
    }
}

fn answer(id: u64, outcome: AckOutcome) {
    let replies = {
        let mut taken = TAKEN.lock();
        match taken.insert(id, (Instant::now(), Taken::Answered(outcome.clone()))) {
            Some((_, Taken::Running(replies))) => replies,
            _ => Vec::new(),
        }
    };
    for reply in replies {
        reply(outcome.clone());
    }
}

/// A command has arrived under `id`, to be answered through `reply`. The
/// first time, it is to be acted on: `Some`, to finish once it has been. A
/// repeat is not: `None`, and `reply` gets the first one's outcome, now if
/// there is one, else when it is finished.
pub fn accept(id: u64, reply: impl FnOnce(AckOutcome) + Send + 'static) -> Option<Pending> {
    let mut taken = TAKEN.lock();
    taken.retain(|_, (at, _)| at.elapsed() < REMEMBERED);
    match taken.get_mut(&id) {
        Some((_, Taken::Running(replies))) => {
            replies.push(Box::new(reply));
            None
        }
        Some((_, Taken::Answered(outcome))) => {
            let outcome = outcome.clone();
            drop(taken);
            reply(outcome);
            None
        }
        None => {
            taken.insert(id, (Instant::now(), Taken::Running(vec![Box::new(reply)])));
            Some(Pending {
                id,
                finished: false,
            })
        }
    }
}

/// Answer a command that wants one through `reply`, and hand it on to be
/// acted on, with what to finish, unless it is a repeat. Without an id it is
/// acted on as it always was.
pub fn take(
    envelope: Envelope,
    reply: impl FnOnce(u64, AckOutcome) + Send + 'static,
) -> Option<(LinkCommand, Option<Pending>)> {
    match envelope.ack {
        None => Some((envelope.command, None)),
        Some(id) => {
            let reply = Arc::new(Mutex::new(Some(reply)));
            let pending = accept(id, move |outcome| {
                if let Some(reply) = reply.lock().take() {
                    reply(id, outcome);
                }
            })?;
            Some((envelope.command, Some(pending)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_envelope_is_the_command_with_an_id_beside_it() {
        let pause = Envelope {
            command: LinkCommand::Pause,
            ack: Some(17),
        };
        let json = serde_json::to_string(&pause).unwrap();
        assert_eq!(json, r#"{"type":"pause","ack":17}"#);
        assert_eq!(serde_json::from_str::<Envelope>(&json).unwrap(), pause);
        // A client that predates acks reads the command and ignores the id.
        assert_eq!(
            serde_json::from_str::<LinkCommand>(&json).unwrap(),
            LinkCommand::Pause
        );
        // And a plain command is an envelope without one.
        assert_eq!(
            serde_json::from_str::<Envelope>(r#"{"type":"pause"}"#).unwrap(),
            Envelope::from(LinkCommand::Pause)
        );
    }

    #[test]
    fn ids_are_random_not_counted() {
        let ids: Vec<u64> = (0..8).map(|_| next_id()).collect();
        assert!(ids.iter().all(|id| *id != 0));
        let consecutive = ids.windows(2).filter(|w| w[1] == w[0] + 1).count();
        assert_eq!(consecutive, 0, "one id says nothing of the next: {ids:?}");
    }

    #[test]
    fn a_repeat_is_answered_with_the_first_outcome_and_not_acted_on() {
        let id = next_id();
        let answers = Arc::new(Mutex::new(Vec::new()));
        let record = |n: u32| {
            let answers = answers.clone();
            move |outcome: AckOutcome| answers.lock().push((n, outcome))
        };

        let first = accept(id, record(1)).expect("acted on");
        // Arriving by another way while the first is still running.
        assert!(accept(id, record(2)).is_none(), "not acted on twice");
        assert!(answers.lock().is_empty());

        first.finish(AckOutcome::Done);
        assert_eq!(
            *answers.lock(),
            [(1, AckOutcome::Done), (2, AckOutcome::Done)]
        );

        // And after it finished: answered at once.
        assert!(accept(id, record(3)).is_none());
        assert_eq!(answers.lock().last(), Some(&(3, AckOutcome::Done)));
    }

    #[test]
    fn a_command_dropped_unfinished_is_answered_as_failed() {
        let id = next_id();
        let answer = Arc::new(Mutex::new(None));
        let got = answer.clone();
        drop(accept(id, move |o| *got.lock() = Some(o)));
        assert!(matches!(*answer.lock(), Some(AckOutcome::Failed { .. })));
    }

    #[test]
    fn a_sender_hears_its_answer_once() {
        let id = next_id();
        let rx = expect(id, "phone");
        resolve(id, "phone", AckOutcome::Queued);
        resolve(id, "phone", AckOutcome::Done);
        assert_eq!(rx.try_recv(), Ok(AckOutcome::Queued));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn only_the_device_a_command_went_to_answers_it() {
        let id = next_id();
        let rx = expect(id, "phone");
        // Another device on the network, which saw the id go by.
        resolve(id, "stranger", AckOutcome::Done);
        assert!(rx.try_recv().is_err(), "a stranger's answer is not heard");
        resolve(id, "phone", AckOutcome::Failed { error: "no".into() });
        assert!(matches!(rx.try_recv(), Ok(AckOutcome::Failed { .. })));
    }
}
