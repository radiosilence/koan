//! A window's root: its content as a cached view, beside which the full
//! theme places the wash, the two side by side as ARCHITECTURE.md describes,
//! so a frame of the wash never lays the page out again.

use gpui_kit::prelude::*;
use gpui_kit::*;

pub struct WindowRoot {
    content: AnyView,
}

impl WindowRoot {
    pub fn new(content: impl Into<AnyView>) -> Self {
        Self {
            content: content.into(),
        }
    }
}

impl Render for WindowRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            self.content
                .clone()
                .cached(StyleRefinement::default().size_full()),
        )
    }
}
