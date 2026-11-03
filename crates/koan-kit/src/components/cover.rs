//! A cover: a record's sleeve, an artist's photo, one tile of a playlist's
//! face. Square unless asked otherwise, filled edge to edge, and while
//! there is no picture the `Placeholder`. What the picture is comes from the
//! caller as `CoverArt`; koan-kit knows nothing of where it was fetched.

use std::sync::Arc;

use gpui::*;

use super::{Animated, Arrival, Placeholder, arriving};
use crate::theme::{Look, Motion};

/// A named cover's arrival: it showed loading, and its picture's fade in.
#[derive(Default)]
struct Landing {
    waited: bool,
    arrival: Option<Arrival>,
}

/// What a cover shows.
#[derive(Clone)]
pub enum CoverArt {
    /// The picture, decoded at the size it is drawn.
    Image(Arc<RenderImage>),
    /// The picture's average colour, standing in while its bitmap uploads:
    /// it already carries the record.
    Colour(Hsla),
    /// Still being fetched or decoded: the placeholder's ensō drawing
    /// itself, on the very mark `Missing` shows.
    Loading,
    /// No picture.
    Missing,
}

#[derive(IntoElement)]
pub struct Cover {
    art: CoverArt,
    size: Size<Pixels>,
    id: Option<ElementId>,
}

impl Cover {
    pub fn new(art: CoverArt, side: Pixels) -> Self {
        Self {
            art,
            size: size(side, side),
            id: None,
        }
    }

    /// Names the cover, so a picture that arrives after it showed loading
    /// fades in over the spinner rather than replacing it.
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// A tile of a mosaic: a cover cut to `size`, its middle kept.
    pub fn sized(art: CoverArt, size: Size<Pixels>) -> Self {
        Self {
            art,
            size,
            id: None,
        }
    }
}

impl RenderOnce for Cover {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let ground = div()
            .w(self.size.width)
            .h(self.size.height)
            .flex_none()
            .overflow_hidden()
            .bg(look.palette.surface.hsla());
        let side = self.size.width.min(self.size.height);
        // Whether this cover showed loading, and when its picture came: the
        // picture fades in over the spinner once, and stands still after.
        let arrived = self
            .id
            .as_ref()
            .map(|id| window.use_keyed_state(id.clone(), cx, |_, _| Landing::default()));
        match self.art {
            CoverArt::Image(image) => {
                // A picture that came after the cover showed loading fades
                // in over the spinner on the GPU; the cover draws once more
                // as it lands, without the spinner.
                let motion = crate::theme::KoanTheme::motion_allowed(cx);
                let fading = arrived.as_ref().and_then(|state| {
                    state.update(cx, |landing, _| {
                        if landing.waited && landing.arrival.is_none() && motion {
                            landing.arrival = Some(Arrival::new(Motion::NORMAL));
                            landing.waited = false;
                        }
                        landing.arrival.clone().filter(|a| !a.landed())
                    })
                });
                match fading {
                    Some(arrival) => {
                        let view = window.current_view();
                        let wait = arrival.lands().saturating_duration_since(Animated::now());
                        cx.spawn(async move |cx| {
                            cx.background_executor().timer(wait).await;
                            cx.update(|cx| cx.notify(view));
                        })
                        .detach();
                        ground
                            .relative()
                            .child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .child(Placeholder::new(side).loading(true)),
                            )
                            .child(div().absolute().inset_0().child(arriving(image, &arrival)))
                    }
                    None => ground.child(
                        img(ImageSource::Render(image))
                            .size_full()
                            .object_fit(ObjectFit::Cover),
                    ),
                }
            }
            CoverArt::Colour(colour) => ground.bg(colour),
            CoverArt::Loading => {
                if let Some(state) = &arrived {
                    state.update(cx, |landing, _| {
                        landing.waited = true;
                        landing.arrival = None;
                    });
                }
                ground.child(Placeholder::new(side).loading(true))
            }
            CoverArt::Missing => ground.child(Placeholder::new(side)),
        }
    }
}

/// A playlist's face: its first four records, square and edge to edge. One
/// fills the square, two split it down the middle, three put the third
/// across the foot; an empty playlist is a plain square of `rule`.
#[derive(IntoElement)]
pub struct Mosaic {
    arts: Vec<CoverArt>,
    side: Pixels,
}

impl Mosaic {
    pub fn new(arts: Vec<CoverArt>, side: Pixels) -> Self {
        Self { arts, side }
    }
}

impl RenderOnce for Mosaic {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let side = self.side;
        let half = side / 2.;
        let tile = |art: &CoverArt, w: Pixels, h: Pixels| Cover::sized(art.clone(), size(w, h));
        let pair = |a: &CoverArt, b: &CoverArt| {
            div()
                .flex()
                .child(tile(a, half, half))
                .child(tile(b, half, half))
        };
        let square = div().size(side).flex_none().overflow_hidden();
        match self.arts.as_slice() {
            [] => square.bg(look.palette.rule.hsla()),
            [one] => square.child(tile(one, side, side)),
            [a, b] => square
                .flex()
                .child(tile(a, half, side))
                .child(tile(b, half, side)),
            [a, b, c] => square
                .flex()
                .flex_col()
                .child(pair(a, b))
                .child(tile(c, side, half)),
            [a, b, c, d, ..] => square.flex().flex_col().child(pair(a, b)).child(pair(c, d)),
        }
    }
}
