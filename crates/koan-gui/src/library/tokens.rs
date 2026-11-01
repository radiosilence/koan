//! The kōan theme as the library pages draw it: the app's `Paint`, one value
//! per render, and setting text in a role.

use koan_kit::app::*;
// sizes and colours computed at runtime from measurements, not chosen from the theme.
use koan_kit::raw::Hsla;

use crate::theme::{KoanStyled, TypeRole, tokens as theme};

pub use crate::theme::TypeRole as Type;

pub use crate::paint::Paint;

/// Text in a role and colour, on the window's scale.
pub trait Typeset: Styled + Sized {
    fn typeset(self, p: &Paint, role: TypeRole, color: Hsla) -> Self {
        self.text_role(role, &p.look).text_color(color)
    }
}

impl<T: Styled> Typeset for T {}

pub fn hairline() -> Pixels {
    theme::HAIRLINE
}
