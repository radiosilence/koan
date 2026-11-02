//! The library page's controls in the window's toolbar: its filter field,
//! then filters, sort, a new deal and select, as the Mac's toolbar carries
//! them. The shell draws the slot in the toolbar's trailing items; the page
//! on screen fills it, and its controls redraw whenever the page does.

use std::rc::Rc;

use koan_kit::app::*;

type Build = Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>;

#[derive(Default)]
pub struct ControlsSlot {
    build: Option<Build>,
    _page: Option<Subscription>,
    _browse: Option<Subscription>,
}

impl ControlsSlot {
    /// The controls of `page`, as its `controls` draws them.
    pub fn show<V: 'static>(
        &mut self,
        page: &Entity<V>,
        controls: fn(&mut V, &mut Window, &mut Context<V>) -> AnyElement,
        cx: &mut Context<Self>,
    ) {
        let weak = page.downgrade();
        self.build = Some(Rc::new(move |window, cx| {
            weak.update(cx, |page, cx| controls(page, window, cx))
                .unwrap_or_else(|_| div().into_any_element())
        }));
        self._page = Some(cx.observe(page, |_, _, cx| cx.notify()));
        self._browse = None;
        cx.notify();
    }

    /// The controls of `page`, redrawn only when what they show moves:
    /// whether it is picking (`picking`), and the browse state their filter
    /// and sort buttons read. A page notifies for every sleeve that lands
    /// while it scrolls, which leaves its controls as they were.
    pub fn show_keyed<V: 'static>(
        &mut self,
        page: &Entity<V>,
        controls: fn(&mut V, &mut Window, &mut Context<V>) -> AnyElement,
        picking: fn(&V) -> bool,
        browse: &Entity<super::browse::Browse>,
        cx: &mut Context<Self>,
    ) {
        let weak = page.downgrade();
        self.build = Some(Rc::new(move |window, cx| {
            weak.update(cx, |page, cx| controls(page, window, cx))
                .unwrap_or_else(|_| div().into_any_element())
        }));
        let mut was = picking(page.read(cx));
        self._page = Some(cx.observe(page, move |_, page, cx| {
            let now = picking(page.read(cx));
            if now != was {
                was = now;
                cx.notify();
            }
        }));
        self._browse = Some(cx.observe(browse, |_, _, cx| cx.notify()));
        cx.notify();
    }

    /// A page with no controls of its own.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        if self.build.take().is_some() {
            self._page = None;
            self._browse = None;
            cx.notify();
        }
    }
}

impl Render for ControlsSlot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let controls = self.build.clone().map(|build| build(window, cx));
        div().flex().items_center().children(controls)
    }
}
