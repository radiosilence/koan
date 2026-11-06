//! Appearance: text size, icons, and the record's colours. There is one
//! look, so no theme to choose. Each is saved by the engine in the
//! background (`[appearance]`) and applied here from the app's own copy.

use crate::components::{Slider, Toggle};
use crate::theme::tokens::{TEXT_SIZE_STEP, TEXT_SIZES};
use crate::theme::{KoanTheme, Look};
use koan_kit::app::*;

use crate::settings::Ctx;
use crate::settings::form::{Row, page, section};

/// The sliders' ends, in characters: the longest word among them ("twice as
/// fast"), so the pane's rails start and stop level with each other.
const ENDS: usize = 13;

pub struct AppearancePane {
    ctx: Ctx,
    /// A slider's value while it is dragged, applied when let go: a new
    /// text size draws every window again, and a new tint bakes the wash.
    text_size: Option<u32>,
    wash_tint: Option<u32>,
    /// The wash's pace and turn while their sliders are dragged.
    wash_drift: Option<u32>,
    wash_turn: Option<u32>,
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
            wash_drift: None,
            wash_turn: None,
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

    /// The wash's pace and turn as dragged, into the theme and the config.
    fn commit_drift(&mut self, cx: &mut Context<Self>) {
        if self.wash_drift.is_none() && self.wash_turn.is_none() {
            return;
        }
        let theme = cx.global::<KoanTheme>();
        let (drift, turned) = (self.wash_drift.take(), self.wash_turn.take());
        let pace = drift.unwrap_or(theme.wash_drift);
        let turn = turned.unwrap_or(theme.wash_turn);
        KoanTheme::set_wash_drift(pace, turn, cx);
        // Only the setting that moved is written.
        let engine = self.ctx.model.read(cx).engine();
        if drift.is_some() {
            engine.set_wash_drift(pace as u16);
        }
        if turned.is_some() {
            engine.set_wash_turn(turn as u8);
        }
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

/// The wash's sliders, each shown as it is dragged and written when let go.
#[derive(Clone, Copy)]
enum Held {
    Tint,
    Drift,
    Turn,
}

impl Held {
    /// Its id, the name a screen reader gives it, its ends, its range and
    /// the step it lands on.
    fn spec(
        self,
    ) -> (
        &'static str,
        &'static str,
        (&'static str, &'static str),
        u32,
        u32,
    ) {
        match self {
            Held::Tint => ("wash-tint", "tint", ("full colour", "held"), 100, 5),
            Held::Drift => ("wash-drift", "drift", ("still", "twice as fast"), 200, 10),
            Held::Turn => ("wash-turn", "turn", ("none", "10°"), 10, 1),
        }
    }

    fn label(self, value: u32) -> String {
        match self {
            Held::Tint => format!("Theme tint {value}%"),
            Held::Drift => format!("Drift {value}%"),
            Held::Turn => format!("Turn {value}°"),
        }
    }
}

impl AppearancePane {
    fn held(&mut self, which: Held) -> &mut Option<u32> {
        match which {
            Held::Tint => &mut self.wash_tint,
            Held::Drift => &mut self.wash_drift,
            Held::Turn => &mut self.wash_turn,
        }
    }

    fn commit(&mut self, which: Held, cx: &mut Context<Self>) {
        match which {
            Held::Tint => self.commit_wash_tint(cx),
            Held::Drift | Held::Turn => self.commit_drift(cx),
        }
    }
}

/// One of the wash's rows: its label with the value, and its slider.
fn held_row(which: Held, value: u32, entity: &Entity<AppearancePane>) -> AnyElement {
    let (id, name, (low, high), max, step) = which.spec();
    let (moving, landing) = (entity.clone(), entity.clone());
    Row::new(which.label(value))
        .stacked()
        .control(
            div().w_full().child(
                Slider::new(id, value as f32 / max as f32)
                    .named(name)
                    .ends(low, high)
                    .ends_width(ENDS)
                    .step(step as f32 / max as f32)
                    .on_change(move |f, _, cx| {
                        let to = snapped(*f, 0, max, step);
                        moving.update(cx, |pane, cx| {
                            let held = pane.held(which);
                            if *held != Some(to) {
                                *held = Some(to);
                                // The wash shows it as it is dragged; the
                                // setting is written when let go.
                                let theme = cx.global::<KoanTheme>();
                                let (pace, turn) = (theme.wash_drift, theme.wash_turn);
                                match which {
                                    Held::Tint => KoanTheme::set_wash_tint(to, cx),
                                    Held::Drift => KoanTheme::set_wash_drift(to, turn, cx),
                                    Held::Turn => KoanTheme::set_wash_drift(pace, to, cx),
                                }
                                cx.notify();
                            }
                        })
                    })
                    .on_commit(move |_, _, cx| {
                        landing.update(cx, |pane, cx| pane.commit(which, cx))
                    }),
            ),
        )
        .into_any_element()
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
        let wash_drift = self.wash_drift.unwrap_or(theme.wash_drift);
        let wash_turn = self.wash_turn.unwrap_or(theme.wash_turn);
        let record_colours = theme.record_colours;
        let wash_window = theme.wash_window;
        let entity = cx.entity();

        let size_slider = Slider::new("text-size", fraction(text_size, low, high))
            .ends(format!("{low}%"), format!("{high}%"))
            .ends_width(ENDS)
            .step(TEXT_SIZE_STEP as f32 / (high - low) as f32)
            .on_change({
                let entity = entity.clone();
                move |f, _, cx| {
                    let percent = snapped(*f, low, high, TEXT_SIZE_STEP);
                    entity.update(cx, |pane, cx| {
                        if pane.text_size != Some(percent) {
                            pane.text_size = Some(percent);
                            // Shown as it is dragged; written when let go.
                            KoanTheme::set_text_size(percent, cx);
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
                .stacked()
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
        // A phone's shell has no sidebar or toolbar ground to wash or not.
        if record_colours && !crate::mobile::phone() {
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
        }
        if record_colours {
            // The tint, then how the wash moves while music plays.
            for (which, value) in [
                (Held::Tint, wash_tint),
                (Held::Drift, wash_drift),
                (Held::Turn, wash_turn),
            ] {
                colour_rows.push(held_row(which, value, &entity));
            }
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
