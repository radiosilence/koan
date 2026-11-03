//! A record's tile in a grid, as the album browser and an artist's page draw
//! it: the grid's geometry (`TileGrid`), the sleeve and what lies over it
//! (`TileArt`), and the measures a tile's owner hit-tests its parts by, since
//! a scrolling grid's tile is one interactive element rather than one per
//! part.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::prelude::*;
use gpui_kit::*;

use super::Placeholder;
#[cfg(test)]
use super::{TileColumns, TileGrid};
#[cfg(test)]
use crate::theme::tokens::Scale;
use crate::theme::{FONT_FAMILY, Icon, KoanIcon, KoanStyled, Look, TypeRole};

/// The sleeve as a tile shows it.
pub enum TileSleeve {
    /// The picture, fading in by `arrival` over `ground`, the record's
    /// colour shown before it, or over the loading ensō where no colour
    /// stood in.
    Image {
        image: Arc<RenderImage>,
        arrival: Option<super::Arrival>,
        ground: Option<Hsla>,
    },
    /// The picture's average colour, standing in while it uploads.
    Colour(Hsla),
    /// On its way: the ensō drawing itself.
    Loading,
    /// No picture, or none to be had: the ensō at rest.
    Missing,
}

/// What dims the sleeve: playing it, under the pointer, or starting to.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TileMark {
    Play,
    Starting,
}

/// The sleeve square and everything over it: the codec top right and the
/// release's badge top left, the play mark over a dim, the playing bars
/// bottom left, the share on this device as a bar along the foot, the heart
/// on its disc bottom right, and while picking the tick and the picked
/// outline. The sleeve keeps its place and size whatever lies over it.
#[derive(IntoElement)]
pub struct TileArt {
    side: Pixels,
    sleeve: TileSleeve,
    codec: Option<SharedString>,
    badge: Option<SharedString>,
    mark: Option<TileMark>,
    playing: Option<AnyElement>,
    on_device: Option<f32>,
    heart: Option<(bool, bool, bool)>,
    id: ElementId,
    pick: Option<bool>,
    sheen: Option<(ElementId, bool)>,
    cursor: bool,
    spot: Option<super::CursorSpot>,
}

impl TileArt {
    /// The heart's disc, in points, and its inset from the corner.
    pub const HEART: f32 = 30.0;
    pub const HEART_INSET: f32 = 7.0;
    const BADGE_INSET: f32 = 6.0;
    const BARS_INSET: f32 = 8.0;
    const BAR: f32 = 3.0;
    /// The pick tick's glyph, in points as drawn.
    const TICK: f32 = 20.0;
    /// What the play mark dims the sleeve with.
    const DIM: f32 = 0.35;

    /// `id` names which record's art this is, for its heart's answer: tiles
    /// drawn side by side in one view each need their own.
    pub fn new(id: impl Into<ElementId>, side: Pixels, sleeve: TileSleeve) -> Self {
        Self {
            side,
            sleeve,
            codec: None,
            badge: None,
            mark: None,
            playing: None,
            on_device: None,
            heart: None,
            id: id.into(),
            pick: None,
            sheen: None,
            cursor: false,
            spot: None,
        }
    }

    /// Which record's art this is, for its heart's answer: tiles drawn
    /// side by side in one view each need their own.
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = id.into();
        self
    }

    /// The codec, top right: at tile size a rate is unreadable, and the
    /// codec is what says whether this is the good copy.
    pub fn codec(mut self, codec: Option<SharedString>) -> Self {
        self.codec = codec;
        self
    }

    /// The release's badge, top left ("deluxe").
    pub fn badge(mut self, badge: Option<SharedString>) -> Self {
        self.badge = badge;
        self
    }

    pub fn mark(mut self, mark: Option<TileMark>) -> Self {
        self.mark = mark;
        self
    }

    /// The playing bars, bottom left, while the record plays.
    pub fn playing(mut self, bars: Option<AnyElement>) -> Self {
        self.playing = bars;
        self
    }

    /// How much of the record is on this device, 0 to 1, while that is
    /// worth showing: a bar along the foot.
    pub fn on_device(mut self, fraction: Option<f32>) -> Self {
        self.on_device = fraction;
        self
    }

    /// The heart: whether it is a favourite, whether the pointer is on the
    /// heart's disc, and whether it shows (under the pointer, or for a
    /// favourite). A heart not showing still follows the favourite, and
    /// shows while it answers a change made elsewhere.
    pub fn heart(mut self, favourite: bool, on_disc: bool, shows: bool) -> Self {
        self.heart = Some((favourite, on_disc, shows));
        self
    }

    /// While picking: whether this record is picked.
    pub fn pick(mut self, picked: Option<bool>) -> Self {
        self.pick = picked;
        self
    }

    /// The rainbow appearance's sheen over the sleeve, under the id `id`
    /// (brat's palette when `brat`): for the tile the pointer is on.
    pub fn sheen(mut self, sheen: Option<(ElementId, bool)>) -> Self {
        self.sheen = sheen;
        self
    }

    /// Under the grid's keyboard cursor while the grid holds the keyboard:
    /// the focus ring, inside the sleeve, once the keyboard was used last.
    pub fn cursor(mut self, cursor: bool) -> Self {
        self.cursor = cursor;
        self
    }

    /// Where the cursor's ring tells its page it was laid out, while this
    /// is under the cursor: what the page's scroll area follows and the menu
    /// key opens under.
    pub fn spot(mut self, spot: &super::CursorSpot) -> Self {
        self.spot = Some(spot.clone());
        self
    }

    /// Whether `(x, y)`, from the sleeve's top left, is on the heart's disc.
    pub fn on_heart(side: Pixels, x: f32, y: f32) -> bool {
        let side = f32::from(side);
        let disc = side - Self::HEART_INSET - Self::HEART..side - Self::HEART_INSET;
        disc.contains(&x) && disc.contains(&y)
    }
}

impl RenderOnce for TileArt {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let pal = &look.palette;
        let side = self.side;
        let mut art = div()
            .relative()
            .size(side)
            .flex_none()
            .bg(pal.surface.hsla());
        match self.sleeve {
            TileSleeve::Image {
                image,
                arrival,
                ground,
            } => {
                // Arriving: over the record's colour where it stood in, or
                // over the ensō that was drawing itself, on the GPU.
                match arrival {
                    Some(arrival) => {
                        art = match ground {
                            Some(ground) => art.bg(ground),
                            None => art.child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .child(Placeholder::new(side).loading(true)),
                            ),
                        };
                        art = art.child(
                            div()
                                .absolute()
                                .inset_0()
                                .child(super::arriving(image, &arrival)),
                        );
                    }
                    None => art = art.child(img(ImageSource::Render(image)).size_full()),
                }
            }
            TileSleeve::Colour(colour) => art = art.bg(colour),
            TileSleeve::Loading => art = art.child(Placeholder::new(side).loading(true)),
            TileSleeve::Missing => art = art.child(Placeholder::new(side)),
        }
        if let Some((id, brat)) = self.sheen {
            art = art.child(super::Sheen::new(id).brat(brat));
        }
        let badge = |text: SharedString| {
            div()
                .absolute()
                .top(px(Self::BADGE_INSET))
                .px(px(Self::BADGE_INSET))
                .py(px(2.0))
                .bg(pal.surface.hsla())
                .text_role(TypeRole::Fine, &look)
                .text_color(pal.ink.hsla())
                .child(text)
        };
        if let Some(codec) = self.codec {
            art = art.child(badge(codec).right(px(Self::BADGE_INSET)));
        }
        if let (Some(release), None) = (self.badge, self.pick) {
            art = art.child(badge(release).left(px(Self::BADGE_INSET)));
        }
        if let Some(mark) = self.mark {
            art = art.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(gpui_kit::black().opacity(Self::DIM))
                    .child(match mark {
                        TileMark::Starting => div()
                            .text_role(TypeRole::TitleSmall, &look)
                            .text_color(pal.strong.hsla())
                            .child("…")
                            .into_any_element(),
                        TileMark::Play => KoanIcon::new(Icon::PlayMark)
                            .role(TypeRole::TitleSmall)
                            .color(pal.strong.hsla())
                            .into_any_element(),
                    }),
            );
        }
        if let Some(bars) = self.playing {
            art = art.child(
                div()
                    .absolute()
                    .left(px(Self::BARS_INSET))
                    .bottom(px(Self::BARS_INSET))
                    .child(bars),
            );
        }
        if let Some(fraction) = self.on_device {
            art = art.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(px(Self::BAR))
                    .bg(pal.rule.hsla())
                    .child(
                        div()
                            .h_full()
                            .w(relative(fraction.clamp(0.0, 1.0)))
                            .bg(look.accent.hsla()),
                    ),
            );
        }
        let heart = self
            .heart
            .filter(|_| self.pick.is_none())
            .filter(|&(favourite, _, shows)| {
                shows
                    || super::Heart::answering(
                        super::child(&self.id, "heart"),
                        favourite,
                        window,
                        cx,
                    )
            });
        if let Some((favourite, hovered, _)) = heart {
            // A mark inside the tile, which takes the click itself and tells
            // the heart from the sleeve by where it lands.
            let glyph = super::Heart::new(super::child(&self.id, "heart"), favourite)
                .rest(crate::theme::Tone::Ink);
            art = art.child(
                div()
                    .absolute()
                    .right(px(Self::HEART_INSET))
                    .bottom(px(Self::HEART_INSET))
                    .size(px(Self::HEART))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(if hovered {
                        pal.hover.hsla()
                    } else {
                        pal.surface.hsla()
                    })
                    .child(glyph),
            );
        }
        if let Some(picked) = self.pick {
            // Over the art rather than a border on it, so ticking a tile
            // neither shrinks its sleeve nor moves it.
            if picked {
                art = art.child(
                    div()
                        .absolute()
                        .inset_0()
                        .border_3()
                        .border_color(look.accent.hsla()),
                );
            }
            // White over a fill, the accent when picked.
            let tick = if picked {
                KoanIcon::new(Icon::Picked).fill(look.accent.hsla())
            } else {
                KoanIcon::new(Icon::Unpicked).fill(gpui_kit::black().opacity(0.25))
            };
            art = art.child(
                div()
                    .absolute()
                    .top(px(Self::HEART_INSET))
                    .left(px(Self::HEART_INSET))
                    .child(tick.points(Self::TICK / 1.2).color(gpui_kit::white())),
            );
        }
        if self.cursor && window.last_input_was_keyboard() {
            art = art.child(super::cursor_ring(&look, self.spot.as_ref()));
        }
        art
    }
}

/// The widths `texts` take side by side in `role`, as drawn: for telling
/// apart which of a line's names the pointer is on.
pub fn text_widths(
    texts: &[SharedString],
    role: TypeRole,
    window: &Window,
    cx: &App,
) -> Vec<Pixels> {
    let look = Look::of(window, cx);
    let size = look.font_size(role);
    let font = font(FONT_FAMILY);
    texts
        .iter()
        .map(|text| {
            let run = TextRun {
                len: text.len(),
                font: font.clone(),
                color: Hsla::default(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            window
                .text_system()
                .shape_line(text.clone(), size, &[run], None)
                .width
        })
        .collect()
}

/// An element filling its parent that records where the parent was laid
/// out, for an owner hit-testing parts of it by position.
pub fn origin_probe(origin: Rc<Cell<Point<Pixels>>>) -> impl IntoElement {
    canvas(
        move |bounds, _, _| origin.set(bounds.origin),
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn tiles_keep_their_size_and_the_room_between_them_takes_the_width() {
        let scale = Scale::of(100);
        let narrow = TileGrid::for_width(px(300.0), &scale);
        assert_eq!(narrow.columns, 1);
        // One tile stands on the margin too, not centred.
        assert_eq!(
            narrow.inset,
            crate::components::RowMetrics::page_margin_on(&scale)
        );
        let a = TileGrid::for_width(px(1200.0), &scale);
        let b = TileGrid::for_width(px(1240.0), &scale);
        assert_eq!(
            a.side, b.side,
            "a resize moves tiles apart, never grows them"
        );
        assert_eq!(a.columns, b.columns);
        assert!(b.gap > a.gap);
        // The grid's edges stand on the page's margin, the header's.
        let margin = crate::components::RowMetrics::page_margin_on(&scale);
        assert_eq!((a.inset, b.inset), (margin, margin));
        assert!(a.gap >= px(TileColumns::MIN_GAP));
        for (grid, width) in [(a, 1200.0), (b, 1240.0)] {
            let used = grid.side * grid.columns as f32
                + grid.gap * (grid.columns as f32 - 1.0)
                + grid.inset * 2.0;
            assert!((f32::from(used) - width).abs() < 1.0);
        }
        // Wide enough for one more tile at the least gap: a column more.
        let more = TileGrid::for_width(a.side * 2.0 + a.gap * 3.0 + px(2000.0), &scale);
        assert!(more.columns > a.columns);
        assert_eq!(a.rows(0), 0);
        assert_eq!(a.rows(a.columns + 1), 2);
    }
}
