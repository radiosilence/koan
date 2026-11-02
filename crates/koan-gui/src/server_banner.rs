//! The server this device cannot use, and why, across the top of every page
//! for as long as that holds. Other devices see this one as unreachable and
//! their commands are refused, so the reason belongs on its own screen.
//! Offline by choice shows nothing; the engine decides (`ServerStanding`).
//!
//! A view of its own: the Connection slice moves with every device that
//! comes and goes, and this redraws only when the standing does.

use koan_ffi::ServerStanding;
use koan_kit::app::*;

use crate::components::Banner;
use crate::mirror::Mirror;
use crate::theme::{Icon, Tone};

pub struct ServerBanner {
    standing: ServerStanding,
    _connection: Subscription,
}

impl ServerBanner {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let connection = cx.global::<Mirror>().connection.clone();
        let standing = |c: &Entity<crate::mirror::Slice<Option<koan_ffi::ConnectionInfo>>>,
                        cx: &App| {
            c.read(cx)
                .0
                .as_ref()
                .map_or(ServerStanding::Fine, |c| c.standing)
        };
        Self {
            standing: standing(&connection, cx),
            _connection: cx.observe(&connection, move |this, connection, cx| {
                let now = standing(&connection, cx);
                if now != this.standing {
                    this.standing = now;
                    cx.notify();
                }
            }),
        }
    }
}

impl Render for ServerBanner {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let message = match self.standing {
            ServerStanding::Fine => return div().into_any_element(),
            ServerStanding::SignedOut => "Signed out of your server",
            ServerStanding::Refused => "Your server refused kōan's sign-in",
            ServerStanding::Unreachable => "Can't reach your server. kōan keeps trying.",
        };
        let unreachable = self.standing == ServerStanding::Unreachable;
        let banner = Banner::new(
            if unreachable {
                Icon::Offline
            } else {
                Icon::Warning
            },
            message,
            Tone::Bad,
        );
        if unreachable {
            return banner.into_any_element();
        }
        banner
            .action("sign in", |_, _, cx| crate::settings::open_server(cx))
            .into_any_element()
    }
}
