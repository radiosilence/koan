//! Keeping the session saved, so an unclean exit costs a second rather than
//! the queue.
//!
//! As the Mac app's `PlayerModel` does: while the music plays, the position is
//! written every second, and the queue too when it has changed since it was
//! last written; the moment it stops is written once; a queue edited while
//! nothing plays is written as it changes. A stopped koan sets no timer.

use std::time::Duration;

use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::engine::engine;
use crate::mirror::{Mirror, Playhead, Queue};

pub struct Session {
    playhead: Entity<Playhead>,
    queue: Entity<Queue>,
    /// The queue version last written whole.
    saved_version: u64,
    playing: bool,
    autosave: Option<Task<()>>,
    _subscriptions: [Subscription; 2],
}

/// Holds the session's entity for the life of the app.
struct Kept(#[allow(dead_code)] Entity<Session>);

impl Global for Kept {}

impl Session {
    pub fn follow(cx: &mut App) {
        let mirror = cx.global::<Mirror>().clone();
        let session = cx.new(|cx| {
            let saved_version = mirror.queue.read(cx).version;
            Self {
                _subscriptions: [
                    cx.observe(&mirror.playhead, |this: &mut Session, _, cx| {
                        this.playhead_moved(cx)
                    }),
                    cx.observe(&mirror.queue, |this: &mut Session, _, cx| {
                        this.queue_moved(cx)
                    }),
                ],
                playhead: mirror.playhead,
                queue: mirror.queue,
                saved_version,
                playing: false,
                autosave: None,
            }
        });
        cx.set_global(Kept(session));
    }

    fn playhead_moved(&mut self, cx: &mut Context<Self>) {
        let playing = self.playhead.read(cx).playing;
        if playing == self.playing {
            return;
        }
        self.playing = playing;
        if playing {
            self.autosave = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    if this.update(cx, |this, cx| this.save(cx)).is_err() {
                        return;
                    }
                }
            }));
        } else {
            self.autosave = None;
            self.save(cx);
        }
    }

    fn queue_moved(&mut self, cx: &mut Context<Self>) {
        if !self.playing && self.queue.read(cx).version != self.saved_version {
            self.save(cx);
        }
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let version = self.queue.read(cx).version;
        let engine = engine(cx);
        let whole = version != self.saved_version;
        self.saved_version = version;
        cx.background_executor()
            .spawn(async move {
                let written = if whole {
                    engine.save_session().await
                } else {
                    engine.save_position().await
                };
                if let Err(e) = written {
                    log::warn!("session: not saved: {e}");
                }
            })
            .detach();
    }
}
