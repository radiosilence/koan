//! The questions an import stops on, wherever it was started: what several
//! files will become, at what rate bare coefficients are, and what an
//! imported EQ is for. Each is the model's, drawn over the EQ page showing on
//! a desktop, and over everything on a phone, where a share from another app
//! can stop on one from any tab.

use koan_kit::app::input::InputState;
use koan_kit::app::{
    AnyElement, App, Entity, IntoElement, MouseButton, ParentElement, SharedString, Window, div,
};
use koan_kit::app::{InteractiveElement, Styled};
use koan_runtime::DspRole;

use super::model::{Dsp, RATES, RoleAsk, Stage, khz};
use super::sheet::{body, choice_row, content, head, note, panel, section};
use crate::components::sheet::scrim;
use crate::components::{Button, ButtonKind, Rise, SectionHeader, TextField};
use crate::theme::{KoanStyled, Look, Space, Tone, TypeRole, label};

pub fn render(dsp: &Entity<Dsp>, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    let d = dsp.read(cx);
    let look = Look::of(window, cx);
    let shown = if !d.pending_imports.is_empty() {
        import_plan(dsp, &look, window, cx)
    } else if d.needs_rate.is_some() {
        rate(dsp, &look)
    } else {
        let ask = d.ask_role.clone()?;
        role(dsp, ask, &look, cx)
    };
    Some(
        scrim("dsp-prompt", &look, window)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(Rise::new(panel("eq", &look).child(shown)))
            .into_any_element(),
    )
}

/// Several files chosen to import, shown before they are: whole presets
/// become a group, parts of one profile combine into one, and either is
/// named here.
fn import_plan(dsp: &Entity<Dsp>, look: &Look, window: &mut Window, cx: &mut App) -> AnyElement {
    let Some(pending) = dsp.read(cx).pending_imports.front().cloned() else {
        return div().into_any_element();
    };
    let plan = pending.plan;
    let key: SharedString = format!("import-name-{}", plan.files.join("\u{0}")).into();
    let input = window.use_keyed_state(key, cx, {
        let name = plan.name.clone();
        move |window, cx| InputState::new(window, cx).default_value(name)
    });
    let name = input.read(cx).value().trim().to_owned();
    let cancel = {
        let dsp = dsp.clone();
        Button::new("import-cancel", ButtonKind::Standard)
            .label("Cancel")
            .on_click(move |_, _, cx| dsp.update(cx, |d, cx| d.cancel_import(cx)))
    };
    let go = {
        let (dsp, input) = (dsp.clone(), input.clone());
        Button::new("import-go", ButtonKind::Standard)
            .label("Import")
            .disabled(name.is_empty())
            .on_click(move |_, _, cx| {
                let name = input.read(cx).value().trim().to_owned();
                if !name.is_empty() {
                    dsp.update(cx, |d, cx| d.confirm_import(name, cx));
                }
            })
    };
    let count = plan.files.len();
    let (heading, footer) = if plan.group {
        (
            format!("These {count} presets will become a group"),
            "Each becomes an EQ of its own, and the group holds them. A group plays one at a time: the first, until you pick another on the group's page or in an output's preset menu.",
        )
    } else {
        (
            format!("These {count} files will be combined into one EQ"),
            "They are parts of one setup, such as a response for each channel, so they play together as one EQ.",
        )
    };
    let files = plan
        .files
        .iter()
        .map(|f| {
            div()
                .text_role(TypeRole::Meta, look)
                .text_color(look.palette.ink.hsla())
                .child(f.clone())
                .into_any_element()
        })
        .collect();
    content()
        .child(head(
            "Import",
            vec![cancel.into_any_element(), go.into_any_element()],
            look,
        ))
        .child(
            body("import-body", look)
                .child(section(Some(&heading), files, Some(footer.into()), look))
                .child(SectionHeader::new(if plan.group {
                    "Group name"
                } else {
                    "Name"
                }))
                .child(TextField::new(&input)),
        )
        .into_any_element()
}

fn rate(dsp: &Entity<Dsp>, look: &Look) -> AnyElement {
    let cancel = {
        let dsp = dsp.clone();
        Button::new("rate-cancel", ButtonKind::Standard)
            .label("Cancel")
            .on_click(move |_, _, cx| dsp.update(cx, |d, cx| d.cancel_rate(cx)))
    };
    let rates = RATES.iter().map(|&r| {
        let dsp = dsp.clone();
        Button::new(("rate", r as usize), ButtonKind::Bordered)
            .label(format!("{} kHz", khz(r)))
            .on_click(move |_, _, cx| dsp.update(cx, |d, cx| d.retry(r, cx)))
    });
    content()
        .child(head("What sample rate is it at?", vec![cancel.into_any_element()], look))
        .child(
            body("rate-body", look)
                .child(note(
                    "These coefficients carry no rate of their own. Use the one the filter was designed at.",
                    Tone::Muted,
                    look,
                ))
                .child(div().flex().flex_wrap().gap(look.space(Space::M)).children(rates)),
        )
        .into_any_element()
}

/// What an imported EQ is for, asked once it is in: kōan cannot tell from a
/// file whether it corrects headphones or speakers, does that with a sound
/// added, or is taste for on top. Each answer says what it means, and for an
/// import started from a device's chain, where it goes.
fn role(dsp: &Entity<Dsp>, ask: RoleAsk, look: &Look, cx: &App) -> AnyElement {
    let many = ask.names.len() > 1;
    let what = if many {
        "kōan can't tell from the files what these EQs do.".to_owned()
    } else {
        format!(
            "kōan can't tell from the file what {} does.",
            ask.names
                .first()
                .cloned()
                .unwrap_or_else(|| "this EQ".into())
        )
    };
    let into = ask.into.as_ref().map(|into| {
        let device = dsp.read(cx).label(&into.device);
        match into.stage {
            Stage::Eq => format!(
                "A tuning is added to {device}'s filters; a correction becomes its correction."
            ),
            Stage::Correction => format!("A correction becomes {device}'s correction."),
        }
    });
    let choice = |id: &'static str, role: DspRole, title: &str, example: &str| {
        let (dsp, ask) = (dsp.clone(), ask.clone());
        choice_row(
            id,
            label(title),
            Some(example.to_owned().into()),
            None,
            look,
            move |_, _, cx| {
                let ask = ask.clone();
                dsp.update(cx, |d, cx| d.answer(ask, Some(role), cx));
            },
        )
        .into_any_element()
    };
    let later = {
        let (dsp, ask) = (dsp.clone(), ask.clone());
        Button::new("role-later", ButtonKind::Link)
            .label("Decide Later")
            .on_click(move |_, _, cx| {
                let ask = ask.clone();
                dsp.update(cx, |d, cx| d.answer(ask, None, cx));
            })
    };
    content()
        .child(head(if many { "What are these EQs?" } else { "What is this EQ?" }, Vec::new(), look))
        .child(
            body("role-body", look)
                .child(div().text_role(TypeRole::Body, look).text_color(look.palette.ink.hsla()).child(what))
                .children(into.map(|i| note(i, Tone::Muted, look)))
                .child(choice(
                    "role-correction",
                    DspRole::Correction,
                    "Correction",
                    "Makes headphones or speakers sound neutral: an AutoEQ result, or a correction from a measurement such as spinorama's or REW's.",
                ))
                .child(choice(
                    "role-baked",
                    DspRole::Baked,
                    "Correction + Tuning",
                    "Corrects and adds taste in one, as presets named for a sound do, like Qudelix's “Lush”.",
                ))
                .child(choice("role-tuning", DspRole::Tuning, "Tuning", "Taste on top of a correction: more bass, a darker treble."))
                // At its own width, on the text's edge, as the choices are.
                .child(div().flex().child(later))
                .child(note(
                    if many {
                        "Decide later: they're kept as tunings, and you can change each one's role on its page."
                    } else {
                        "Decide later: it's kept as a tuning, and you can change its role on its page."
                    },
                    Tone::Muted,
                    look,
                )),
        )
        .into_any_element()
}
