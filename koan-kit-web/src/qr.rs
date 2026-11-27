//! A QR code to scan off the screen.

use std::fmt::Write as _;

use askama::Template;

use crate::arg::Text;
use crate::attrs::{Attrs, component};

/// `data` as a QR code, for a phone to scan off the screen: `kit::Qr::new(link)?.label("invite link")`.
///
/// Dark on white whatever the scheme, since scanners look for that contrast, inside the four modules of white the
/// specification asks for, and each module a whole square, so the edges stay crisp at any size. The code is drawn from
/// its module matrix by the kit's template, so an app has no SVG of its own to splice into the page.
#[must_use = "a QR code draws nothing until it is rendered"]
#[derive(Clone, Debug, Template)]
#[template(path = "qr.html")]
pub struct Qr<'a> {
    /// The modules a side, the quiet zone included.
    side: usize,
    /// Each row's runs of dark modules as one rectangle each, in SVG path data: figures and path commands alone.
    modules: String,
    label: Text<'a>,
    attrs: Attrs<'a>,
}

/// The modules of white around the code.
const QUIET: usize = 4;

impl Qr<'_> {
    /// The code for `data`, or `None` when it holds more than a QR code can.
    #[must_use]
    pub fn new(data: impl AsRef<[u8]>) -> Option<Self> {
        let code = qrcode::QrCode::new(data).ok()?;
        let width = code.width();
        let colours = code.to_colors();
        let mut modules = String::new();
        for (y, row) in colours.chunks(width).enumerate() {
            let mut x = 0;
            while x < width {
                let run = row[x..]
                    .iter()
                    .take_while(|colour| **colour == qrcode::Color::Dark)
                    .count();
                if run == 0 {
                    x += 1;
                } else {
                    let _ = write!(modules, "M{} {}h{run}v1h-{run}z", x + QUIET, y + QUIET);
                    x += run;
                }
            }
        }
        Some(Self {
            side: width + 2 * QUIET,
            modules,
            label: Text::default(),
            attrs: Attrs::default(),
        })
    }
}

impl<'a> Qr<'a> {
    /// What it carries, for a screen reader: `"invite link"`. Without one it is hidden from screen readers, beside
    /// words or a value that already say it.
    pub fn label(mut self, label: impl Into<Text<'a>>) -> Self {
        self.label = label.into();
        self
    }
}

component!(Qr);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_is_drawn_from_its_modules() {
        let html = Qr::new("https://koan.rocks/join/x")
            .expect("fits")
            .label("invite link")
            .to_string();
        assert!(html.starts_with("<div class=\"qr\""), "{html}");
        assert!(
            html.contains("role=\"img\" aria-label=\"invite link\""),
            "{html}"
        );
        assert!(
            html.contains("<path d=\"M4 4h7v1h-7z"),
            "the finder's top edge is one run of seven: {html}"
        );
        assert!(
            html.len() < 8000,
            "a rectangle per run, not per module: {} bytes",
            html.len()
        );
        assert!(!html.contains("<script"), "{html}");
    }

    #[test]
    fn too_much_for_a_code_is_none() {
        assert!(Qr::new(vec![b'x'; 8000]).is_none());
    }
}
