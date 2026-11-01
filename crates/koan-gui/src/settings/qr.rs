//! A QR code as quads: the code and its runs of dark modules worked out once
//! per link, then painted as one quad per run.

use std::sync::Arc;

use koan_kit::app::*;
// sizes and colours computed at runtime from measurements, not chosen from the theme.
use koan_kit::raw::{point, rgb, size};
// painted and hit-tested by hand: drawing koan-kit has no component for.
use koan_kit::raw::{canvas, fill};

/// A code's dark modules, as horizontal runs: (row, first column, length),
/// on a grid `side` modules square.
#[derive(Clone)]
pub struct Code {
    side: usize,
    runs: Arc<[(u16, u16, u16)]>,
}

impl Code {
    /// The engine's code for `text`, as every app draws it.
    pub fn new(text: &str) -> Option<Code> {
        let code = koan_ffi::qr_code(text.to_owned())?;
        let side = code.width as usize;
        Some(Code {
            side,
            runs: runs(&code.dark, side).into(),
        })
    }

    /// The code drawn `size` across, dark on a light quiet zone of
    /// `margin`, as phones read best whatever the window's appearance.
    pub fn element(&self, side: Pixels, margin: Pixels) -> impl IntoElement + use<> {
        let code = self.clone();
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                window.paint_quad(fill(bounds, rgb(0xFFFFFF)));
                let module = (bounds.size.width - margin * 2.) / code.side as f32;
                let origin = bounds.origin + point(margin, margin);
                for &(row, column, length) in code.runs.iter() {
                    let at = origin + point(module * column as f32, module * row as f32);
                    window.paint_quad(fill(
                        Bounds::new(at, size(module * length as f32, module)),
                        rgb(0x000000),
                    ));
                }
            },
        )
        .size(side)
    }
}

/// Each row's dark modules joined into runs, so a code is a few hundred
/// quads rather than a thousand.
fn runs(dark: &[bool], side: usize) -> Vec<(u16, u16, u16)> {
    let mut runs = Vec::new();
    for row in 0..side {
        let mut column = 0;
        while column < side {
            if dark[row * side + column] {
                let start = column;
                while column < side && dark[row * side + column] {
                    column += 1;
                }
                runs.push((row as u16, start as u16, (column - start) as u16));
            } else {
                column += 1;
            }
        }
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::runs;

    #[test]
    fn runs_cover_every_dark_module_once() {
        let code = koan_ffi::qr_code("https://koan.rocks/join#abc".into()).unwrap();
        let side = code.width as usize;
        let mut painted = vec![false; side * side];
        for (row, column, length) in runs(&code.dark, side) {
            for c in column..column + length {
                let cell = &mut painted[row as usize * side + c as usize];
                assert!(!*cell);
                *cell = true;
            }
        }
        assert_eq!(painted, code.dark);
    }
}
