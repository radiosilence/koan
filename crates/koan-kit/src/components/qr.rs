//! A QR code as quads: its runs of dark modules worked out once, then
//! painted as one quad per run, dark on a light quiet zone whatever the
//! window's appearance, as phones read best.

use std::sync::Arc;

use gpui::*;

/// A code's dark modules, as horizontal runs: (row, first column, length),
/// on a grid `side` modules square.
#[derive(Clone)]
pub struct QrCode {
    side: usize,
    runs: Arc<[(u16, u16, u16)]>,
}

impl QrCode {
    /// From the modules row by row, `side` to a row, `true` where dark. A
    /// module the slice falls short of is light: a short slice draws a code
    /// that does not read, rather than taking the window down.
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
        let at = |column: usize| dark.get(row * side + column).copied().unwrap_or(false);
        while column < side {
            if at(column) {
                let start = column;
                while column < side && at(column) {
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
        // Runs of one, of several, and to a row's end.
        let dark: Vec<bool> = (0..side * side).map(|i| i % 4 != 0).collect();
        let mut painted = vec![false; side * side];
        for (row, column, length) in runs(&dark, side) {
            for c in column..column + length {
                let cell = &mut painted[row as usize * side + c as usize];
                assert!(!*cell);
                *cell = true;
            }
        }
        assert_eq!(painted, dark);
        assert!(runs(&dark, side).iter().any(|&(_, _, length)| length > 1));
    }

    /// Modules short of the grid read as light, rather than out of bounds.
    #[test]
    fn a_short_slice_is_light_where_it_ends() {
        let side = 5;
        let dark = vec![true; side * side - 3];
        let painted: usize = runs(&dark, side).iter().map(|r| r.2 as usize).sum();
        assert_eq!(painted, side * side - 3);
    }
}
