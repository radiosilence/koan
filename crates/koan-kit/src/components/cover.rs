//! A cover: a record's sleeve, an artist's photo, one tile of a playlist's
//! face. Square unless asked otherwise, filled edge to edge, and while
//! there is no picture the `Placeholder`. What the picture is comes from the
//! caller as `CoverArt`; koan-kit knows nothing of where it was fetched.

use std::sync::Arc;

use gpui_kit::*;

use super::Placeholder;
use crate::theme::Look;

/// What a cover shows.
#[derive(Clone)]
pub enum CoverArt {
    /// The picture, decoded at the size it is drawn.
    Image(Arc<RenderImage>),
    /// The picture's average colour, standing in while its bitmap uploads:
    /// it already carries the record.
    Colour(Hsla),
    /// No picture, or none yet.
    Missing,
}

#[derive(IntoElement)]
pub struct Cover {
    art: CoverArt,
    size: Size<Pixels>,
}

impl Cover {
    pub fn new(art: CoverArt, side: Pixels) -> Self {
        Self {
            art,
            size: size(side, side),
        }
    }

    /// A tile of a mosaic: a cover cut to `size`, its middle kept.
    pub fn sized(art: CoverArt, size: Size<Pixels>) -> Self {
        Self { art, size }
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
        match self.art {
            CoverArt::Image(image) => ground.child(
                img(ImageSource::Render(image))
                    .size_full()
                    .object_fit(ObjectFit::Cover),
            ),
            CoverArt::Colour(colour) => ground.bg(colour),
            CoverArt::Missing => {
                ground.child(Placeholder::new(self.size.width.min(self.size.height)))
            }
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
