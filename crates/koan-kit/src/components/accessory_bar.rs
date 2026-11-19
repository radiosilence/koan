//! A bar over the foot of a touch window, above its tabs: something going
//! on that the person can reach from any page.

use std::rc::Rc;

use gpui::{
    AnyElement, App, ElementId, InteractiveElement, IntoElement, ParentElement, Pixels,
    RenderOnce, ScrollEvent, ScrollSource, SharedString, StatefulInteractiveElement, Styled,
    TouchPhase, Window, div,
};

use super::{Glide, ProgressBar};
use crate::theme::{Look, Space};

/// How far a finger pushes the bar up before it opens.
const LIFT: f32 = 24.0;

/// A bar over a touch window's tabs, on the page's ground, laying out what
/// it is given in a row on the page's steps, centred down it, with an
/// optional progress line along its top edge, `AccessoryBar::height` tall.
/// A tap on the bar, or a finger pushing it up, calls `on_open`, which
/// opens what the bar stands for.
///
/// It holds no parts of its own: an app composes it, typically a picture,
/// `TwoLines` and a `ControlGroup`, whose presses stay the controls'.
#[derive(IntoElement)]
pub struct AccessoryBar {
    id: SharedString,
    children: Vec<AnyElement>,
    progress: Option<(f32, Option<Glide>)>,
    on_open: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
}

impl AccessoryBar {
    /// The bar's height, as iOS draws a phone's bar over the tabs: 56
    /// points.
    pub fn height(look: &Look) -> Pixels {
        look.spaced(56.0)
    }

    /// The side of a picture leading the bar, such as a cover: 30 points.
    pub fn picture(look: &Look) -> Pixels {
        look.spaced(30.0)
    }

    /// An empty bar, its gestures kept under `id`.
    pub fn new(id: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            children: Vec::new(),
            progress: None,
            on_open: None,
        }
    }

    /// A line along the top, `share` of the way across (0 to 1), moved on
    /// the GPU between reports by `glide` when one is given.
    pub fn progress(mut self, share: f32, glide: Option<&Glide>) -> Self {
        self.progress = Some((share, glide.cloned()));
        self
    }

    /// What a tap on the bar, or a finger pushing it up, does. Without it
    /// the bar takes neither.
    pub fn on_open(mut self, on_open: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_open = Some(Rc::new(on_open));
        self
    }
}

impl ParentElement for AccessoryBar {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for AccessoryBar {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let id = self.id;
        let progress = self.progress.map(|(share, glide)| {
            let line = ProgressBar::new(
                ElementId::Name(format!("{id}-progress").into()),
                Some(share),
            )
            .thin();
            let line = match &glide {
                Some(glide) => line.glide(glide),
                None => line,
            };
            div().absolute().top_0().left_0().right_0().child(line)
        });
        let mut bar = div()
            .id(ElementId::Name(id.clone()))
            .relative()
            .w_full()
            .h(Self::height(&look))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(look.space(Space::M))
            .pl(look.space(Space::L))
            .pr(look.space(Space::S))
            .bg(look.palette.bg.hsla())
            .children(progress)
            .children(self.children);
        if let Some(open) = self.on_open {
            let lift = window.use_keyed_state(
                ElementId::Name(format!("{id}-lift").into()),
                cx,
                |_, _| None::<Pixels>,
            );
            let tapped = open.clone();
            bar = bar
                .on_click(move |_, window, cx| tapped(window, cx))
                .on_scroll_wheel(move |event: &ScrollEvent, window, cx| {
                    if event.source != ScrollSource::Touch {
                        return;
                    }
                    let dy = event.delta.pixel_delta(Pixels::from(16.)).y;
                    let opened = lift.update(cx, |lift, _| match (event.touch_phase, *lift) {
                        (TouchPhase::Started, _) => {
                            *lift = Some(dy);
                            false
                        }
                        (TouchPhase::Moved, Some(was)) if was + dy < -Pixels::from(LIFT) => {
                            *lift = None;
                            true
                        }
                        (TouchPhase::Moved, Some(was)) => {
                            *lift = Some(was + dy);
                            false
                        }
                        (TouchPhase::Moved, None) => false,
                        _ => {
                            *lift = None;
                            false
                        }
                    });
                    if opened {
                        // The rest of the swipe and its fling are spent on
                        // opening, so none of it scrolls or drags what opens.
                        window.end_touch_scroll();
                        open(window, cx);
                    }
                    cx.stop_propagation();
                });
        }
        bar
    }
}
