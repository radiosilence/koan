//! Appearance: text size, icons, and the record's colours. There is one
//! look, so no theme to choose. Each is saved by the engine in the
//! background (`[appearance]`) and applied here from the app's own copy.

use crate::components::{Slider, Toggle};
use crate::theme::tokens::{TEXT_SIZE_STEP, TEXT_SIZES};
use crate::theme::{KoanTheme, Look};
use koan_kit::raw::prelude::*;
use koan_kit::raw::*;

use crate::settings::Ctx;
use crate::settings::form::{Row, page, section};

pub struct AppearancePane {
    ctx: Ctx,
    /// A slider's value while it is dragged, applied when let go: a new
    /// text size draws every window again, and a new tint bakes the wash.
    text_size: Option<u32>,
    wash_tint: Option<u32>,
    /// A change from elsewhere: the main window's text size, another
    /// window's Settings.
    _theme: Subscription,
}

impl AppearancePane {
    pub fn new(ctx: Ctx, cx: &mut Context<Self>) -> Self {
        Self {
            ctx,
            text_size: None,
            wash_tint: None,
            _theme: cx.observe_global::<KoanTheme>(|_, cx| cx.notify()),
        }
    }

    fn commit_text_size(&mut self, cx: &mut Context<Self>) {
        let Some(percent) = self.text_size.take() else {
            return;
        };
        KoanTheme::set_text_size(percent, cx);
        self.ctx
            .model
            .read(cx)
            .engine()
            .set_text_size(percent as u16);
        cx.notify();
    }

    fn commit_wash_tint(&mut self, cx: &mut Context<Self>) {
        let Some(percent) = self.wash_tint.take() else {
            return;
        };
        KoanTheme::set_wash_tint(percent, cx);
        self.ctx
            .model
            .read(cx)
            .engine()
            .set_wash_tint(percent as u8);
        cx.notify();
    }
}

/// Where a percentage sits on its slider, 0 to 1.
fn fraction(value: u32, low: u32, high: u32) -> f32 {
    (value.saturating_sub(low)) as f32 / (high - low) as f32
}

/// The percentage at `fraction` of a slider, to the nearest step.
fn snapped(fraction: f32, low: u32, high: u32, step: u32) -> u32 {
    let raw = low as f32 + fraction.clamp(0.0, 1.0) * (high - low) as f32;
    let steps = ((raw - low as f32) / step as f32).round() as u32;
    (low + steps * step).min(high)
}

impl Render for AppearancePane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let theme = cx.global::<KoanTheme>();
        let (low, high) = (*TEXT_SIZES.start(), *TEXT_SIZES.end());
        let text_size = self.text_size.unwrap_or(theme.text_size);
        let wash_tint = self.wash_tint.unwrap_or(theme.wash_tint);
        let record_colours = theme.record_colours;
        let wash_window = theme.wash_window;
        let entity = cx.entity();

        let size_slider = Slider::new("text-size", fraction(text_size, low, high))
            .ends(format!("{low}%"), format!("{high}%"))
            .step(TEXT_SIZE_STEP as f32 / (high - low) as f32)
            .on_change({
                let entity = entity.clone();
                move |f, _, cx| {
                    let percent = snapped(*f, low, high, TEXT_SIZE_STEP);
                    entity.update(cx, |pane, cx| {
                        if pane.text_size != Some(percent) {
                            pane.text_size = Some(percent);
                            cx.notify();
                        }
                    })
                }
            })
            .on_commit({
                let entity = entity.clone();
                move |_, _, cx| entity.update(cx, |pane, cx| pane.commit_text_size(cx))
            });

        let engine = self.ctx.model.read(cx).engine().clone();
        let icons_engine = engine.clone();
        let theme_rows = vec![
            Toggle::new("show-icons", "Show icons", look.icons)
                .on_change(move |on, _, cx| {
                    KoanTheme::set_icons(*on, cx);
                    icons_engine.set_theme_icons(*on);
                })
                .into_any_element(),
            Row::new(format!("Text size {text_size}%"))
                .control(div().w_full().child(size_slider))
                .into_any_element(),
        ];

        let mut colour_rows = vec![
            Toggle::new("record-colours", "Colours from the record", record_colours)
                .on_change({
                    let engine = engine.clone();
                    let entity = entity.clone();
                    move |on, _, cx| {
                        KoanTheme::set_record_colours(*on, cx);
                        engine.set_record_colours(*on);
                        entity.update(cx, |_, cx| cx.notify());
                    }
                })
                .into_any_element(),
        ];
        if record_colours {
            colour_rows.push(
                Toggle::new("wash-window", "Wash the whole window", wash_window)
                    .on_change({
                        let engine = engine.clone();
                        let entity = entity.clone();
                        move |on, _, cx| {
                            KoanTheme::set_wash_window(*on, cx);
                            engine.set_wash_window(*on);
                            entity.update(cx, |_, cx| cx.notify());
                        }
                    })
                    .into_any_element(),
            );
            colour_rows.push(
                Row::new(format!("Theme tint {wash_tint}%"))
                    .control(
                        div().w_full().child(
                            Slider::new("wash-tint", wash_tint as f32 / 100.)
                                .ends("full colour", "held")
                                .step(0.05)
                                .on_change({
                                    let entity = entity.clone();
                                    move |f, _, cx| {
                                        let percent = snapped(*f, 0, 100, 5);
                                        entity.update(cx, |pane, cx| {
                                            if pane.wash_tint != Some(percent) {
                                                pane.wash_tint = Some(percent);
                                                cx.notify();
                                            }
                                        })
                                    }
                                })
                                .on_commit(move |_, _, cx| {
                                    entity.update(cx, |pane, cx| pane.commit_wash_tint(cx))
                                }),
                        ),
                    )
                    .into_any_element(),
            );
        }
        let tint_note = if record_colours {
            " Theme tint is how far the wash is darkened in dark mode, or lightened in light, so text over it keeps its contrast: at 0 the sleeve keeps its own colours."
        } else {
            ""
        };

        page(
            "appearance",
            &look,
            vec![
                section(
                    Some("Theme"),
                    theme_rows,
                    Some("Show icons puts icons beside the labels in the sidebar, the tabs and the buttons. Text size scales the type, and its spacing by half as much.".into()),
                    &look,
                ),
                section(
                    Some("Colour"),
                    colour_rows,
                    Some(format!("The record playing colours the window behind the page, and the accent on selection and progress. Off, there is no wash and the accent is kōan's mint.{tint_note}").into()),
                    &look,
                ),
            ],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{fraction, snapped};

    #[test]
    fn a_slider_lands_on_steps() {
        assert_eq!(snapped(0.0, 80, 130, 5), 80);
        assert_eq!(snapped(1.0, 80, 130, 5), 130);
        assert_eq!(snapped(0.4, 80, 130, 5), 100);
        assert_eq!(snapped(0.41, 80, 130, 5), 100);
        assert_eq!(snapped(0.46, 80, 130, 5), 105);
        assert_eq!(fraction(100, 80, 130), 0.4);
    }
}
