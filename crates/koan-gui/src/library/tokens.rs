//! The kōan theme as the library pages draw it: the app's `Paint`, one value
//! per render, and setting text in a role.

use crate::theme::Hsla;
use koan_kit::app::*;

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
