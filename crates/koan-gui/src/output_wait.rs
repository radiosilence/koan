//! The output this device is still waiting for, across the top of every page
//! once it has taken a while (`NowPlaying::output_wait`): a busy audio
//! system or a device waking. The player goes on taking commands meanwhile,
//! and starts from wherever they leave it; the listener can ask again or give
//! up on the device, which plays on the system's default instead.
//!
//! A view of its own: the Playback slice moves with every track and state,
//! and this redraws only when the wait does.

use koan_kit::app::*;
use koan_runtime::OutputWait;

use crate::components::Banner;
use crate::mirror::Mirror;
use crate::theme::{Icon, Tone};

pub struct OutputWaitBanner {
    wait: Option<OutputWait>,
    _playback: Subscription,
}

impl OutputWaitBanner {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let playback = cx.global::<Mirror>().playback.clone();
        Self {
            wait: playback.read(cx).0.output_wait.clone(),
            _playback: cx.observe(&playback, |this, playback, cx| {
                let now = &playback.read(cx).0.output_wait;
                if *now != this.wait {
                    this.wait = now.clone();
                    cx.notify();
                }
            }),
        }
    }
}

impl Render for OutputWaitBanner {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let Some(wait) = &self.wait else {
            return div().into_any_element();
        };
        let device = match &wait.device {
            Some(device) => device.as_str(),
            None => "the system's output",
        };
        // Asking again is refused while an earlier attempt is stuck in the
        // audio system: said instead of offered.
        let (message, retry) = if wait.stuck {
            (
                format!("Waiting for {device}. An earlier attempt is still stuck."),
                false,
            )
        } else {
            (format!("Waiting for {device}…"), true)
        };
        let mut banner = Banner::new(Icon::Speaker, message, Tone::Muted);
        if retry {
            banner = banner.action("retry", |_, _, cx| {
                crate::engine::command(cx, "retry the output", |e| e.retry_output_open())
            });
        }
        banner
            .action("cancel", |_, _, cx| {
                crate::engine::command(cx, "stop waiting for the output", |e| {
                    e.cancel_output_open()
                })
            })
            .into_any_element()
    }
}
