//! A link as a QR code: the engine's code, as every app draws it, drawn by
//! koan-kit's `QrCode`.

use koan_kit::app::*;

use crate::components::QrCode;

#[derive(Clone)]
pub struct Code(QrCode);

impl Code {
    /// The engine's code for `text`.
    pub fn new(text: &str) -> Option<Code> {
        let code = koan_runtime::qr_code(text.to_owned())?;
        Some(Code(QrCode::new(&code.dark, code.width as usize)))
    }

    /// The code drawn `side` across, inside a light quiet zone `margin` wide.
    pub fn element(&self, side: Pixels, margin: Pixels) -> impl IntoElement + use<> {
        self.0.element(side, margin)
    }
}
