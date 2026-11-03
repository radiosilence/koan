//! The sidebar's search field: what is searched for, and a click that
//! opens the search box over the window (`search::bar`) on it. The box is
//! where the query is typed and its answers are listed; ⌘K and `/` open it
//! too.

use koan_kit::app::*;

use super::Search;
use crate::components::FilterButton;
use crate::theme::Space;

pub struct SearchField {
    search: Entity<Search>,
    _search: Subscription,
}

impl SearchField {
    pub fn new(search: Entity<Search>, cx: &mut Context<Self>) -> Self {
        let watch = cx.observe(&search, |_, _, cx| cx.notify());
        Self {
            search,
            _search: watch,
        }
    }
}

impl Render for SearchField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = crate::theme::Look::of(window, cx);
        let query = self.search.read(cx).query().to_string();
        div()
            .w_full()
            .px(look.space(Space::L))
            .py(look.space(Space::S))
            .child(
                FilterButton::new("sidebar-search", query, "search").on_click(|_, window, cx| {
                    window.dispatch_action(crate::platform::actions::Search.boxed_clone(), cx)
                }),
            )
    }
}
