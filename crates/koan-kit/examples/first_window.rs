//! A first window on koan-kit: a page whose header and form come from the
//! kit, a toggle and a choice the view keeps, and a button that puts them
//! back. Nothing in it sets a size, a colour or a gap: the page, the form
//! and the controls own those. The guide's first example
//! (`docs/guide/koan-kit.md`) is this file.
//!
//! cargo run -p koan-kit --example first_window

use koan_kit::app::*;
use koan_kit::components::{
    Button, ButtonKind, Form, FormRow, FormSection, Page, PageHeader, SecondaryWindow, Segmented,
    Toggle,
};
use koan_kit::theme::{self, Look};

/// The view: what the page shows, and nothing about how it looks.
struct Playback {
    shuffle: bool,
    quality: usize,
}

impl Render for Playback {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The tokens resolved for this window: appearance, text size, accent.
        let look = Look::of(window, cx);
        let form = Form::new("playback")
            .section(
                FormSection::titled("playback")
                    .row(
                        Toggle::new("shuffle", "shuffle", self.shuffle).on_change(cx.listener(
                            |this, on: &bool, _, cx| {
                                this.shuffle = *on;
                                cx.notify();
                            },
                        )),
                    )
                    .row(
                        FormRow::new("quality").control(
                            Segmented::new("quality", ["original", "high", "low"], self.quality)
                                .on_change(cx.listener(|this, chosen: &usize, _, cx| {
                                    this.quality = *chosen;
                                    cx.notify();
                                })),
                        ),
                    )
                    .footer("Low quality is used only on a metered connection."),
            )
            .section(
                FormSection::new().row(
                    Button::new("reset", ButtonKind::Bordered)
                        .label("reset")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.shuffle = false;
                            this.quality = 0;
                            cx.notify();
                        })),
                ),
            );
        Page::new(PageHeader::new("playback").detail("a first window")).body(form.build(&look))
    }
}

fn main() {
    koan_kit::app::platform::application()
        // The icon set, which the controls' glyphs are drawn from.
        .with_assets(theme::icons::Assets)
        .run(|cx: &mut App| {
            // The theme, the system's display settings and the kit's keys.
            theme::init(cx);
            let opened = SecondaryWindow::new("Playback", "rocks.koan.first-window")
                .size(640.0, 480.0)
                .open(cx, |_, cx| {
                    cx.new(|_| Playback {
                        shuffle: false,
                        quality: 0,
                    })
                });
            if let Err(e) = opened {
                eprintln!("the window did not open: {e}");
                cx.quit();
            }
        });
}
