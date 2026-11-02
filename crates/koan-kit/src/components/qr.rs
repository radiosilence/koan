//! A QR code as quads: its runs of dark modules worked out once, then
//! painted as one quad per run, dark on a light quiet zone whatever the
//! window's appearance, as phones read best.

use std::sync::Arc;

use gpui_kit::*;

/// A code's dark modules, as horizontal runs: (row, first column, length),
/// on a grid `side` modules square.
#[derive(Clone)]
pub struct QrCode {
    side: usize,
    runs: Arc<[(u16, u16, u16)]>,
}

impl QrCode {
    /// From the modules row by row, `side` to a row, `true` where dark.
    pub fn new(dark: &[bool], side: usize) -> Self {
        Self {
            side,
            runs: runs(dark, side).into(),
        }
    }

    /// The code drawn `side` across, inside a light quiet zone `margin` wide.
    pub fn element(&self, side: Pixels, margin: Pixels) -> impl IntoElement + use<> {
        let code = self.clone();
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                window.paint_quad(fill(bounds, rgb(0xFFFFFF)));
                let module = (bounds.size.width - margin * 2.) / code.side.max(1) as f32;
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
        let side = 5;
        let dark: Vec<bool> = (0..side * side).map(|i| (i * 7) % 3 == 0).collect();
        let mut painted = vec![false; side * side];
        for (row, column, length) in runs(&dark, side) {
            for c in column..column + length {
                let cell = &mut painted[row as usize * side + c as usize];
                assert!(!*cell);
                *cell = true;
            }
        }
        assert_eq!(painted, dark);
    }
}
