//! What the link tests share.

use super::*;

pub(super) fn drain(rx: &mut tokio::sync::mpsc::Receiver<Envelope>) -> Vec<LinkCommand> {
    std::iter::from_fn(|| rx.try_recv().ok().map(|e| e.command)).collect()
}
