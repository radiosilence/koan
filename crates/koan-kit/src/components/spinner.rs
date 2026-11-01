//! Something on its way: eight square marks round a square, the lit one
//! moving on and the others fading behind it. Drawn as quads, and moving
//! only while it is painted: each frame asks for the next, so one scrolled
//! away or behind a page costs nothing. With Reduce Motion it stands still.

use std::time::{Duration, Instant};

use gpui_kit::*;

use crate::theme::{KoanTheme, Look};

/// One turn of the lit mark.
const TURN: Duration = Duration::from_millis(900);
const MARKS: usize = 8;

#[derive(IntoElement)]
pub struct Spinner {
    side: Pixels,
}

impl Spinner {
    /// `side` is the square it is drawn in.
    pub fn new(side: Pixels) -> Self {
        Self { side }
    }
}

/// When the first spinner was drawn: every spinner turns in step.
static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

impl RenderOnce for Spinner {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let ink = look.palette.ink.hsla();
        let moving = KoanTheme::motion_allowed(cx);
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let start = *START.get_or_init(Instant::now);
                let phase = if moving {
                    (start.elapsed().as_secs_f32() / TURN.as_secs_f32()).fract()
                } else {
                    0.0
                };
                let side = bounds.size.width.min(bounds.size.height);
                let mark = side / 5.;
                // The marks round the square's edge, clockwise from the top
                // left: its corners and the middles of its sides.
                let at = [
                    (0., 0.),
                    (0.5, 0.),
                    (1., 0.),
                    (1., 0.5),
                    (1., 1.),
                    (0.5, 1.),
                    (0., 1.),
                    (0., 0.5),
                ];
                let lit = phase * MARKS as f32;
                for (index, (x, y)) in at.into_iter().enumerate() {
                    // How far behind the lit mark this one is, 0 to 1.
                    let behind = (lit - index as f32).rem_euclid(MARKS as f32) / MARKS as f32;
                    let alpha = 0.15 + 0.85 * (1. - behind).powi(2);
                    let origin = bounds.origin + point((side - mark) * x, (side - mark) * y);
                    window.paint_quad(fill(
                        Bounds::new(origin, size(mark, mark)),
                        ink.opacity(alpha),
                    ));
                }
                if moving {
                    window.request_animation_frame();
                }
            },
        )
        .size(self.side)
    }
}
