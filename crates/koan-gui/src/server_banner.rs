//! The server this device cannot use, and why, across the top of every page
//! for as long as that holds. Other devices see this one as unreachable and
//! their commands are refused, so the reason belongs on its own screen.
//! Offline by choice shows nothing; the engine decides (`ServerStanding`).
//!
//! Out of reach with nothing in the queue on this device, the banner says
//! that instead, in the engine's words (`NowPlaying::start_failure`): the
//! one place a phone with no network tells why nothing plays.
//!
//! A view of its own: the Connection slice moves with every device that
//! comes and goes, and the Playback slice with every track, and this redraws
//! only when the standing or the reason does.

use koan_kit::app::*;
use koan_runtime::ServerStanding;

use crate::components::{Banner, Hang};
use crate::mirror::Mirror;
use crate::theme::{Icon, Tone};

pub struct ServerBanner {
    standing: ServerStanding,
    /// Why nothing plays, while the engine says.
    why: Option<String>,
    /// What was showing while it goes back up behind the bar.
    leaving: Option<(ServerStanding, Task<()>)>,
    _connection: Subscription,
    _playback: Subscription,
}

impl ServerBanner {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let connection = cx.global::<Mirror>().connection.clone();
        let playback = cx.global::<Mirror>().playback.clone();
        let standing = |c: &Entity<crate::mirror::Slice<Option<koan_runtime::ConnectionInfo>>>,
                        cx: &App| {
            c.read(cx)
                .0
                .as_ref()
                .map_or(ServerStanding::Fine, |c| c.standing)
        };
        Self {
            standing: standing(&connection, cx),
            why: playback.read(cx).0.start_failure.clone(),
            leaving: None,
            _playback: cx.observe(&playback, |this, playback, cx| {
                let why = &playback.read(cx).0.start_failure;
                if *why != this.why {
                    this.why = why.clone();
                    cx.notify();
                }
            }),
            _connection: cx.observe(&connection, move |this, connection, cx| {
                let now = standing(&connection, cx);
                if now != this.standing {
                    let was = std::mem::replace(&mut this.standing, now);
                    let moves = crate::theme::KoanTheme::motion_allowed(cx);
                    this.leaving = (now == ServerStanding::Fine && moves).then(|| {
                        let done = cx.spawn(async |this, cx| {
                            cx.background_executor().timer(Hang::LEAVE).await;
                            this.update(cx, |this, cx| {
                                this.leaving = None;
                                cx.notify();
                            })
                            .ok();
                        });
                        (was, done)
                    });
                    cx.notify();
                }
            }),
        }
    }
}

/// What the banner says, if anything. Unreachable, the engine's reason for
/// a wait no download can end takes the place of the plain message; any
/// other reason a track did not start is not the server's, and is not said
/// here.
fn message(standing: ServerStanding, why: Option<&str>) -> Option<SharedString> {
    Some(match (standing, why) {
        (ServerStanding::Fine, _) => return None,
        (ServerStanding::SignedOut, _) => "Signed out of your server".into(),
        (ServerStanding::Refused, _) => "Your server refused kōan's sign-in".into(),
        (ServerStanding::Unreachable, Some(why))
            if why == koan_runtime::NOTHING_HERE || why == koan_runtime::NOT_HERE =>
        {
            SharedString::from(why.to_owned())
        }
        (ServerStanding::Unreachable, _) => "Can't reach your server. kōan keeps trying.".into(),
    })
}

impl Render for ServerBanner {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let leaving = self.leaving.as_ref().map(|(was, _)| *was);
        let Some(message) = message(leaving.unwrap_or(self.standing), self.why.as_deref()) else {
            return div().into_any_element();
        };
        let unreachable = leaving.unwrap_or(self.standing) == ServerStanding::Unreachable;
        let banner = Banner::new(
            if unreachable {
                Icon::Offline
            } else {
                Icon::Warning
            },
            message,
            Tone::Bad,
        );
        let banner = if unreachable {
            banner
        } else {
            banner.action("sign in", |_, _, cx| crate::settings::open_server(cx))
        };
        Hang::new(banner)
            .leaving(leaving.is_some())
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreachable_says_why_nothing_plays_and_nothing_else() {
        let plain = "Can't reach your server. kōan keeps trying.";
        let said = |why| message(ServerStanding::Unreachable, why).map(|m| m.to_string());
        assert_eq!(said(None).as_deref(), Some(plain));
        assert_eq!(
            said(Some(koan_runtime::NOTHING_HERE)).as_deref(),
            Some(koan_runtime::NOTHING_HERE)
        );
        assert_eq!(
            said(Some("Couldn't play this track: device busy")).as_deref(),
            Some(plain),
            "a failure that is not the server's is not said here"
        );
        assert_eq!(
            message(ServerStanding::Fine, Some(koan_runtime::NOT_HERE)),
            None
        );
    }
}
