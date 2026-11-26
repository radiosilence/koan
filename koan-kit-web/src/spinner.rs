//! The loader: the ensō drawing itself, as koan-kit's Spinner draws it, wherever something is on its way.

use std::fmt;

use crate::arg::Text;
use crate::attrs::{Attrs, component, escape};

/// The ensō turning: `kit::Spinner::new().label("loading the album")`.
///
/// Its head runs round and its tail follows it off, a turn every 1.6 seconds, in the colour of the text around it;
/// under reduced motion the whole mark stands still. Drawn at the size of the text beside it, [`large`](Self::large)
/// for a wait that fills a region, and [`over`](Self::over) centred over the nearest positioned ancestor, as over a
/// video that is buffering. Without a [`label`](Self::label) it is decoration beside words that already say what is
/// loading; with one it is a status a screen reader announces.
#[must_use = "a spinner draws nothing until it is rendered"]
#[derive(Clone, Debug, Default)]
pub struct Spinner<'a> {
    label: Text<'a>,
    large: bool,
    over: bool,
    attrs: Attrs<'a>,
}

impl<'a> Spinner<'a> {
    /// At the size of the text beside it, hidden from screen readers.
    pub fn new() -> Self {
        Self::default()
    }

    /// What is on its way, announced to a screen reader and not shown: `"loading the album"`.
    pub fn label(mut self, label: impl Into<Text<'a>>) -> Self {
        self.label = label.into();
        self
    }

    /// Two lines tall, for a wait that fills a region: a media player, a page loading.
    pub fn large(mut self) -> Self {
        self.large = true;
        self
    }

    /// Centred over the nearest positioned ancestor, taking no room of its own and letting presses through.
    pub fn over(mut self) -> Self {
        self.over = true;
        self
    }
}

impl fmt::Display for Spinner<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<span class=\"spinner")?;
        if self.large {
            f.write_str(" large")?;
        }
        if self.over {
            f.write_str(" over")?;
        }
        write!(f, "{}\"", self.attrs.classes())?;
        match self.label.present() {
            Some(label) => {
                write!(f, " role=\"status\"{}><span class=\"label\">", self.attrs)?;
                escape(f, label)?;
                f.write_str("</span></span>")
            }
            None => write!(f, " aria-hidden=\"true\"{}></span>", self.attrs),
        }
    }
}

component!(Spinner);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spinner_is_decoration_until_it_says_what_is_loading() {
        assert_eq!(
            Spinner::new().to_string(),
            "<span class=\"spinner\" aria-hidden=\"true\"></span>"
        );
        assert_eq!(
            Spinner::new().large().over().label("buffering").to_string(),
            "<span class=\"spinner large over\" role=\"status\"><span class=\"label\">buffering</span></span>"
        );
    }
}
