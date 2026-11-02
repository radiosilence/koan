//! EQ: what the output in use plays, and the way into the EQ window, where
//! profiles are imported, made and chosen for each device.

use koan_ffi::DspOverview;
use koan_kit::app::*;

use crate::components::{Button, ButtonKind};
use crate::mirror::Mirror;
use crate::settings::Ctx;
use crate::settings::form::{Row, buttons, page, section};
use crate::theme::Look;

pub struct EqPane {
    ctx: Ctx,
    overview: Option<DspOverview>,
    reading: Option<Task<()>>,
    /// The library version the overview was last read at.
    library: u64,
    _subscriptions: [Subscription; 2],
}

impl EqPane {
    pub fn new(ctx: Ctx, cx: &mut Context<Self>) -> Self {
        let mirror = cx.global::<Mirror>().clone();
        let mut pane = Self {
            _subscriptions: [
                // The output and its preset are published with the outputs,
                // the profiles synced with the library: read again when its
                // version moves, not at every notification a scan makes.
                cx.observe(&mirror.outputs, |pane, _, cx| pane.read(cx)),
                cx.observe(&mirror.library, |pane, library, cx| {
                    let version = library.read(cx).version;
                    if version != pane.library {
                        pane.library = version;
                        pane.read(cx);
                    }
                }),
            ],
            library: mirror.library.read(cx).version,
            ctx,
            overview: None,
            reading: None,
        };
        pane.read(cx);
        pane
    }

    fn read(&mut self, cx: &mut Context<Self>) {
        let engine = self.ctx.model.read(cx).engine().clone();
        self.reading = Some(cx.spawn(async move |this, cx| {
            let overview = engine.dsp_overview().await;
            this.update(cx, |pane, cx| {
                pane.overview = Some(overview);
                cx.notify();
            })
            .ok();
        }));
    }
}

/// What the output plays, in words: its preset, or none.
fn playing(overview: &DspOverview) -> String {
    if !overview.enabled {
        return "Nothing: processing is off".into();
    }
    let device = overview.device.as_deref();
    let state = overview
        .outputs
        .iter()
        .find(|o| Some(o.device.as_str()) == device);
    match (state.and_then(|s| s.preset.as_ref()), state) {
        (Some(preset), Some(s)) if s.preset_edited => format!("{preset}, edited"),
        (Some(preset), _) => preset.clone(),
        (None, Some(s)) if !s.flat => "Unsaved".into(),
        _ => "No EQ".into(),
    }
}

impl Render for EqPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let Some(overview) = &self.overview else {
            return page("eq", &look, Vec::new());
        };
        let device = overview.device.clone();
        let name = device
            .as_ref()
            .map(|d| overview.names.get(d).cloned().unwrap_or_else(|| d.clone()))
            .unwrap_or_else(|| "None".into());
        let mut rows = vec![
            Row::new("Output").value(name).into_any_element(),
            Row::new("Plays")
                .value(playing(overview))
                .into_any_element(),
            Row::new("Profiles")
                .value(overview.profiles.len().to_string())
                .into_any_element(),
        ];
        if !overview.enabled {
            rows.push(Row::new("Processing").value("Off").into_any_element());
        }
        let open = Button::new("open-eq", ButtonKind::Standard)
            .label("Open EQ…")
            // The output in use, followed as it changes, as ⌥⌘E opens it.
            .on_click(move |_, _, cx| crate::dsp::window::open(None, cx));
        page(
            "eq",
            &look,
            vec![
                section(
                    Some("Output in use"),
                    rows,
                    Some("Profiles are imported, made and chosen for each device in the EQ window: its chain, its preset and the curve of the whole. The transport's EQ button switches presets.".into()),
                    &look,
                ),
                buttons(vec![open.into_any_element()], &look),
            ],
        )
    }
}
