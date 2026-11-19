//! What stands over the foot of a touch window's pages: the foot itself,
//! the player over the tabs (`TabFoot`), and a card floating above it
//! (`FootCard`).

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, InteractiveElement, IntoElement, ParentElement, Pixels, Point, RenderOnce,
    SharedString, Styled, Window, div,
};

use super::{Clear, FadeIn, Shifted, TransportMetrics, foot_inset, foot_probe, system_insets};
use crate::theme::{Look, Space};

/// A card over the foot of the pages, a step above what the shell draws
/// there, with the page's margin either side: what the app is busy with,
/// where a phone has no sidebar foot to hold it. Even padding on every
/// side, as the iOS app's card. It rises into place, fades in, and fades
/// out while `leaving`.
#[derive(IntoElement)]
pub struct FootCard {
    id: SharedString,
    child: gpui::AnyElement,
    leaving: bool,
}

impl FootCard {
    /// A card holding `child`, its motion kept under `id`.
    pub fn new(id: impl Into<SharedString>, child: impl IntoElement) -> Self {
        Self {
            id: id.into(),
            child: child.into_any_element(),
            leaving: false,
        }
    }

    /// Fading out, on its way off the page.
    pub fn leaving(mut self, leaving: bool) -> Self {
        self.leaving = leaving;
        self
    }
}

impl RenderOnce for FootCard {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let card = Shifted::new(
            self.id.clone(),
            Point::new(Pixels::ZERO, look.space(Space::Xl)),
            div()
                .p(look.space(Space::M))
                .bg(look.palette.surface.hsla())
                .child(self.child),
        );
        div()
            .absolute()
            .left(look.space(Space::M))
            .right(look.space(Space::M))
            .bottom(foot_inset(window, cx) + look.space(Space::S))
            .child(
                FadeIn::new(SharedString::from(format!("{}-in", self.id)), card)
                    .leaving(self.leaving),
            )
    }
}

/// The foot a touch shell draws over its pages, running on under the home
/// indicator and measured for the pages' foot inset (`foot_probe`). A
/// phone's (`phone`) is the mini player, its playhead along the foot's
/// edge, over the tabs under a rule, solid: only the bar at the top is
/// frosted. A tablet's (`tablet`) is the full transport, frosted as the
/// desktop's is.
#[derive(IntoElement)]
pub struct ShellFoot {
    player: AnyElement,
    tabs: Option<AnyElement>,
}

impl ShellFoot {
    /// A phone's foot: `player` at `TransportMetrics::mini_height`, over
    /// `tabs`.
    pub fn phone(player: impl IntoElement, tabs: impl IntoElement) -> Self {
        Self {
            player: player.into_any_element(),
            tabs: Some(tabs.into_any_element()),
        }
    }

    /// A tablet's foot: `player` at `TransportMetrics::HEIGHT`, frosted.
    pub fn tablet(player: impl IntoElement) -> Self {
        Self {
            player: player.into_any_element(),
            tabs: None,
        }
    }
}

impl RenderOnce for ShellFoot {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let insets = system_insets(Clear::System, window);
        let phone = self.tabs.is_some();
        div()
            .id(if phone { "phone-foot" } else { "tablet-foot" })
            .absolute()
            .bottom_0()
            .left_0()
            .right_0()
            .occlude()
            .pb(insets.bottom)
            .map(|foot| {
                if phone {
                    foot.bg(look.palette.bg.hsla())
                } else {
                    foot.child(super::Frost::foot())
                }
            })
            .child(foot_probe())
            .child(
                div()
                    .w_full()
                    .h(if phone {
                        TransportMetrics::mini_height(&look)
                    } else {
                        TransportMetrics::HEIGHT
                    })
                    .flex_shrink_0()
                    .child(self.player),
            )
            .children(self.tabs.map(|tabs| {
                div()
                    .border_t(look.hairline)
                    .border_color(look.palette.rule.hsla())
                    .child(tabs)
            }))
    }
}
