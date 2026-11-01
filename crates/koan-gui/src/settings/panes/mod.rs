//! The Settings panes, one file each. Each is a view of its own, rendered
//! cached under the window's thin root, so it observes every entity it
//! draws from.

pub mod account;
mod appearance;
mod cache;
mod devices;
mod eq;
pub mod integrations;
mod keys;
mod library;
mod link;
pub mod people;
mod playback;
mod server;
mod share;

use crate::theme::Icon;
use gpui_kit::*;

use super::Ctx;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Library,
    Server,
    Account,
    People,
    Playback,
    Eq,
    Devices,
    Integrations,
    Appearance,
}

impl Tab {
    pub fn title(self) -> &'static str {
        match self {
            Tab::Library => "Library",
            Tab::Server => "Server",
            Tab::Account => "Account",
            Tab::People => "People",
            Tab::Playback => "Playback",
            Tab::Eq => "EQ",
            Tab::Devices => "Devices",
            Tab::Integrations => "Integrations",
            Tab::Appearance => "Appearance",
        }
    }

    pub fn icon(self) -> Icon {
        match self {
            Tab::Library => Icon::Library,
            Tab::Server => Icon::Server,
            Tab::Account => Icon::Account,
            Tab::People => Icon::People,
            Tab::Playback => Icon::Speaker,
            Tab::Eq => Icon::Tuning,
            Tab::Devices => Icon::Devices,
            Tab::Integrations => Icon::Extensions,
            Tab::Appearance => Icon::Appearance,
        }
    }
}

pub struct Pane(AnyView);

impl Pane {
    pub fn new(
        tab: Tab,
        ctx: &Ctx,
        people: Option<Entity<people::People>>,
        window: &mut Window,
        cx: &mut App,
    ) -> Pane {
        let ctx = ctx.clone();
        Pane(match tab {
            Tab::Library => cx.new(|cx| library::LibraryPane::new(ctx, cx)).into(),
            Tab::Server => cx.new(|cx| server::ServerPane::new(ctx, window, cx)).into(),
            Tab::Account => cx
                .new(|cx| account::AccountPane::new(ctx, window, cx))
                .into(),
            Tab::People => match people {
                Some(people) => cx
                    .new(|cx| people::PeoplePane::new(ctx, people, window, cx))
                    .into(),
                None => cx.new(|cx| library::LibraryPane::new(ctx, cx)).into(),
            },
            Tab::Playback => cx.new(|cx| playback::PlaybackPane::new(ctx, cx)).into(),
            Tab::Eq => cx.new(|cx| eq::EqPane::new(ctx, cx)).into(),
            Tab::Devices => cx
                .new(|cx| devices::DevicesPane::new(ctx, window, cx))
                .into(),
            Tab::Integrations => cx
                .new(|cx| integrations::IntegrationsPane::new(ctx, window, cx))
                .into(),
            Tab::Appearance => cx.new(|cx| appearance::AppearancePane::new(ctx, cx)).into(),
        })
    }

    pub fn view(&self) -> AnyView {
        self.0.clone()
    }
}
