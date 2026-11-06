//! EQ and convolution: profiles per output device, the response graph and
//! its editor, the band table, and every way a profile comes in. The page
//! is `EqPage`, Settings' EQ pane on the desktop and on a phone; everything
//! else opens over it as a sheet.

pub mod bands;
pub mod flows;
pub mod graph;
pub mod manage;
pub mod menu;
pub mod model;
pub mod page;
pub mod profile;
pub mod prompts;
pub mod sheet;
pub mod stage;

use koan_kit::app::{App, Entity, Global};

pub use model::Dsp;
pub use page::EqPage;

use crate::platform::actions::ShowEq;

/// The EQ model, made the first time EQ is shown and shared by every page
/// drawing it, so an edit or an import outlives the page it began on.
struct Model(Entity<Dsp>);

impl Global for Model {}

/// The EQ model.
pub fn model(cx: &mut App) -> Entity<Dsp> {
    if let Some(model) = cx.try_global::<Model>() {
        return model.0.clone();
    }
    let dsp = Dsp::new(crate::engine::engine(cx), cx);
    cx.set_global(Model(dsp.clone()));
    dsp
}

/// Answers Playback → EQ…. Call once the engine and the mirror are
/// installed.
pub fn init(cx: &mut App) {
    cx.on_action(|_: &ShowEq, cx| crate::settings::open_eq(None, cx));
}
