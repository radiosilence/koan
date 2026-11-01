//! Playback: the output device, the transport's fade, and loudness.

use crate::components::{FormRow, Picker, Stepper, Toggle};
use crate::theme::Look;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::mirror::{Mirror, Outputs};
use crate::settings::Ctx;
use crate::settings::form::{page, section};

const REPLAYGAIN: [(&str, &str); 3] = [
    ("Off", "off"),
    ("Per track", "track"),
    ("Per album", "album"),
];

pub struct PlaybackPane {
    ctx: Ctx,
    /// This device's audio outputs, and the one kept in the config (`None`
    /// for the system's default). Read again when the outputs change, as a
    /// device is plugged in or out.
    devices: Vec<String>,
    current: Option<String>,
    reading: Option<Task<()>>,
    _subscriptions: [Subscription; 2],
}

impl PlaybackPane {
    pub fn new(ctx: Ctx, cx: &mut Context<Self>) -> Self {
        let outputs: Entity<Outputs> = cx.global::<Mirror>().outputs.clone();
        let mut pane = Self {
            _subscriptions: [
                cx.observe(&ctx.model, |_, _, cx| cx.notify()),
                cx.observe(&outputs, |pane, _, cx| pane.read_devices(cx)),
            ],
            ctx,
            devices: Vec::new(),
            current: None,
            reading: None,
        };
        pane.read_devices(cx);
        pane
    }

    fn read_devices(&mut self, cx: &mut Context<Self>) {
        let engine = self.ctx.model.read(cx).engine().clone();
        self.reading = Some(cx.spawn(async move |this, cx| {
            let devices = engine.clone().devices().await.unwrap_or_default();
            let current = engine.current_device().await;
            this.update(cx, |pane, cx| {
                pane.devices = devices.into_iter().map(|d| d.name).collect();
                pane.current = current;
                cx.notify();
            })
            .ok();
        }));
    }

    fn set_device(&mut self, name: Option<String>, cx: &mut Context<Self>) {
        self.current = name.clone();
        cx.notify();
        let engine = self.ctx.model.read(cx).engine().clone();
        let model = self.ctx.model.clone();
        cx.spawn(async move |this, cx| {
            let result = match name {
                Some(name) => engine.set_device(name).await,
                None => engine.clear_device().await,
            };
            if let Err(e) = result {
                model.update(cx, |model, cx| {
                    model.report(crate::settings::model::describe(&e), cx)
                });
            }
            this.update(cx, |pane, cx| pane.read_devices(cx)).ok();
        })
        .detach();
    }
}

impl Render for PlaybackPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let Some(settings) = self.ctx.model.read(cx).settings() else {
            return page("playback", &look, Vec::new());
        };
        let entity = cx.entity();
        let model = self.ctx.model.clone();

        let mut outputs = vec!["System Default".to_owned()];
        outputs.extend(self.devices.iter().cloned());
        let selected = match &self.current {
            None => Some(0),
            Some(name) => self.devices.iter().position(|d| d == name).map(|i| i + 1),
        };
        let devices = self.devices.clone();
        let device = FormRow::new("Output").control(
            Picker::keeps_case("output", outputs, selected).on_change(move |index, _, cx| {
                let name = index.checked_sub(1).and_then(|i| devices.get(i).cloned());
                entity.update(cx, |pane, cx| pane.set_device(name, cx));
            }),
        );

        let fade = {
            let model = model.clone();
            Toggle::new("fade-on-pause", "Fade on pause", settings.fade_on_pause).on_change(
                move |on, _, cx| {
                    let on = *on;
                    model.update(cx, |m, cx| m.edit(cx, move |s| s.fade_on_pause = on));
                },
            )
        };

        let mut loudness = vec![
            FormRow::new("ReplayGain")
                .control({
                    let model = model.clone();
                    Picker::new(
                        "replaygain",
                        REPLAYGAIN.map(|(label, _)| label),
                        REPLAYGAIN
                            .iter()
                            .position(|(_, v)| *v == settings.replaygain),
                    )
                    .on_change(move |index, _, cx| {
                        let mode = REPLAYGAIN[*index].1;
                        model.update(cx, |m, cx| m.edit(cx, move |s| s.replaygain = mode.into()));
                    })
                })
                .into_any_element(),
        ];
        if settings.replaygain != "off" {
            let half_steps = (settings.pre_amp_db * 2.).round() as i64;
            loudness.push(
                Stepper::new("pre-amp", "Pre-amp", half_steps, -30..=30)
                    .shown(format!("{:.1} dB", settings.pre_amp_db))
                    .on_change(move |steps, _, cx| {
                        let db = *steps as f64 / 2.;
                        model.update(cx, |m, cx| m.edit(cx, move |s| s.pre_amp_db = db));
                    })
                    .into_any_element(),
            );
        }

        page(
            "playback",
            &look,
            vec![
                section(
                    Some("Device"),
                    vec![device.into_any_element()],
                    Some("kōan asks the device to run at the source's sample rate, so nothing is resampled unless the device refuses.".into()),
                    &look,
                ),
                section(
                    Some("Transport"),
                    vec![fade.into_any_element()],
                    Some("Pause and resume ramp the volume over a moment instead of cutting.".into()),
                    &look,
                ),
                section(
                    Some("Loudness"),
                    loudness,
                    Some("Applies the gain written into the file's tags. Per album keeps the relative loudness within a record.".into()),
                    &look,
                ),
            ],
        )
    }
}
