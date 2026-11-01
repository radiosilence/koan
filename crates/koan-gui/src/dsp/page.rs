//! EQ for a device, as the chain it plays: music in, its correction, its
//! filters' EQs in order, the device out. The device and its preset head the
//! page, then the curve of the whole chain, always the same height, then the
//! chain itself. A device set to play filters alone has no correction stage,
//! and nothing on the page speaks of targets.

use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, AppContext, Context, Entity, ExternalPaths, InteractiveElement, IntoElement,
    ParentElement, PathPromptOptions, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, Task, Window, canvas, div, point, px,
};
use koan_ffi::{AutoEqOffer, DspEqJoin, DspJoin, DspOverview, DspRole, DspTuningEntry};

use super::graph::{Graph, Part, Stroke};
use super::menu::{Item, Trigger, menu};
use super::model::{Dsp, Placement, Stage, describe_profile, joined};
use super::sheet::{Confirm, Sheets, note};
use super::{flows, manage, prompts, stage};
use crate::components::{Button, ButtonKind, FormRow, SectionHeader, TextField, Toggle};
use crate::theme::{
    HAIRLINE, Icon, KoanIcon, KoanStyled, Look, MARK, Space, Tone, TypeRole, label,
};

pub struct EqPage {
    dsp: Entity<Dsp>,
    pub sheets: Entity<Sheets>,
    /// The device shown: the output in use until another is picked.
    picked: Option<String>,
    overview: Option<DspOverview>,
    /// The curve each stage draws alone, by profile name.
    curves: HashMap<String, Vec<f64>>,
    graph: Entity<Graph>,
    /// What the overview was last read at, and the read under way.
    loaded: Option<((u64, u64), Option<String>)>,
    _load: Option<Task<()>>,
    /// A name being asked for a new preset, and why the last was refused.
    naming: Option<(SharedString, Entity<InputState>)>,
    refusal: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl EqPage {
    /// The page for `device`, or for the output in use.
    pub fn new(
        dsp: Entity<Dsp>,
        device: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let graph = cx.new(|cx| Graph::new(Graph::flat_response(), cx).height(px(220.0)));
        let sheets = Sheets::new(cx);
        let subscriptions = vec![
            cx.observe(&dsp, |page: &mut EqPage, _, cx| page.follow(cx)),
            cx.observe(&sheets, |_, _, cx| cx.notify()),
        ];
        let _ = window;
        let mut page = Self {
            dsp,
            sheets,
            picked: device,
            overview: None,
            curves: HashMap::new(),
            graph,
            loaded: None,
            _load: None,
            naming: None,
            refusal: None,
            _subscriptions: subscriptions,
        };
        page.follow(cx);
        page
    }

    /// Shows `device`, the output in use with none.
    pub fn show(&mut self, device: Option<String>, cx: &mut Context<Self>) {
        let in_use = self
            .dsp
            .read(cx)
            .overview
            .as_ref()
            .and_then(|o| o.device.clone());
        self.picked = device.filter(|d| Some(d) != in_use.as_ref());
        self.follow(cx);
    }

    fn device(&self) -> Option<String> {
        self.picked
            .clone()
            .or_else(|| self.overview.as_ref().and_then(|o| o.device.clone()))
    }

    /// Reads the device's chain again when the profiles or the device shown
    /// change. The old curve stays until the new one is drawn, so the page
    /// never empties between presets.
    fn follow(&mut self, cx: &mut Context<Self>) {
        let stamp = self.dsp.read(cx).stamp();
        let key = (stamp, self.picked.clone());
        if self.loaded.as_ref() == Some(&key) {
            cx.notify();
            return;
        }
        self.loaded = Some(key);
        let engine = self.dsp.read(cx).engine();
        let picked = self.picked.clone();
        self._load = Some(cx.spawn(async move |this, cx| {
            let o = engine.clone().dsp_overview_for(picked).await;
            let response = match &o.device {
                Some(d) => {
                    engine
                        .clone()
                        .dsp_output_response_for(d.clone(), super::model::GRAPH_RATE)
                        .await
                }
                None => None,
            };
            let mut curves = HashMap::new();
            for name in o.active.iter().chain(o.chain.iter().map(|e| &e.name)) {
                if let Some(r) = engine
                    .clone()
                    .dsp_response(name.clone(), super::model::GRAPH_RATE)
                    .await
                {
                    curves.insert(name.clone(), r.total);
                }
            }
            this.update(cx, |page, cx| {
                let flat = o.active.is_none() && o.chain.is_empty();
                let parts = parts(&o, response.as_ref());
                page.graph.update(cx, |g, cx| {
                    match (response, flat) {
                        (Some(r), false) => {
                            g.set_response(r, stamp, cx);
                            g.set_flat_note(None, cx);
                        }
                        _ => {
                            g.set_response(Graph::flat_response(), stamp, cx);
                            g.set_flat_note(Some("Flat: plays untouched".into()), cx);
                        }
                    }
                    g.set_parts(if flat { Vec::new() } else { parts }, cx);
                });
                page.overview = Some(o);
                page.curves = curves;
                cx.notify();
            })
            .ok();
        }));
    }

    fn flat(&self) -> bool {
        self.overview
            .as_ref()
            .is_none_or(|o| o.active.is_none() && o.chain.is_empty())
    }

    // MARK: - Doing

    /// Opens the file picker for a stage's Import…; what it makes goes into
    /// that stage once its role is answered.
    pub fn import(dsp: Entity<Dsp>, into: Option<Placement>, cx: &mut App) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: None,
        });
        cx.spawn(async move |cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            dsp.update(cx, |dsp, cx| dsp.import_files(paths, None, None, into, cx));
        })
        .detach();
    }

    fn ask_preset_name(&mut self, title: &str, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(label("Name")));
        let sub = cx.subscribe_in(&input, window, |page, _, e: &InputEvent, _, cx| {
            if let InputEvent::PressEnter { .. } = e {
                page.save_named(cx);
            }
        });
        self._subscriptions.push(sub);
        self.naming = Some((title.to_owned().into(), input));
        self.refusal = None;
        cx.notify();
    }

    fn save_named(&mut self, cx: &mut Context<Self>) {
        let Some((_, input)) = &self.naming else {
            return;
        };
        let name = input.read(cx).value().trim().to_owned();
        self.save(name, false, cx);
    }

    /// Saves the chain as the preset `name`. A new name never writes over
    /// another preset: only Save, `over`, changes one.
    fn save(&mut self, name: String, over: bool, cx: &mut Context<Self>) {
        let (Some(device), Some(o)) = (self.device(), &self.overview) else {
            return;
        };
        if name.is_empty() {
            return;
        }
        if !over && o.profiles.iter().any(|p| p.preset && p.name == name) {
            self.refusal = Some(
                format!("There is already a preset called {name}. Choose another name, or change it with Save.").into(),
            );
            cx.notify();
            return;
        }
        self.refusal = None;
        self.naming = None;
        let this = cx.weak_entity();
        self.dsp.update(cx, |dsp, cx| {
            dsp.act_then(
                cx,
                move |engine| async move { engine.dsp_save_preset(device, name).await },
                move |taken, cx| {
                    if taken.is_none() {
                        this.update(cx, |_, cx| cx.notify()).ok();
                    }
                },
            )
        });
        cx.notify();
    }
}

/// Each stage playing, drawn as its block is.
fn parts(o: &DspOverview, r: Option<&koan_ffi::DspResponse>) -> Vec<Part> {
    let Some(r) = r else { return Vec::new() };
    let eqs: Vec<(usize, &str, &Vec<f64>)> = o
        .chain
        .iter()
        .enumerate()
        .filter(|(_, e)| e.on)
        .filter_map(|(i, e)| {
            r.layers
                .iter()
                .find(|l| l.name == e.name)
                .map(|l| (i, e.name.as_str(), &l.db))
        })
        .collect();
    let mut parts = Vec::new();
    if let Some(active) = &o.active
        && let Some(db) = r.correction.as_ref().or(eqs.is_empty().then_some(&r.total))
    {
        parts.push(Part {
            name: active.clone().into(),
            db: db.clone(),
            stroke: Stroke::CORRECTION,
        });
    }
    for (i, name, db) in eqs {
        parts.push(Part {
            name: name.to_owned().into(),
            db: db.clone(),
            stroke: Stroke::eq(i),
        });
    }
    // The total is the left channel's: the right drawn too where an EQ with
    // bands for each channel makes it differ.
    if let Some(right) = &r.right {
        parts.push(Part {
            name: "Right channel".into(),
            db: right.clone(),
            stroke: Stroke::RIGHT,
        });
    }
    parts
}

/// The chain in words, for the page's footer and for a screen reader. What
/// plays: the EQs switched on and not left out, and none on a correction
/// that already includes a tuning.
pub fn sentence(o: &DspOverview, device: &str) -> String {
    let includes = o
        .profiles
        .iter()
        .find(|p| Some(&p.name) == o.active.as_ref())
        .is_some_and(|p| p.role == DspRole::Baked);
    let eqs: Vec<String> = if includes {
        Vec::new()
    } else {
        o.chain
            .iter()
            .filter(|e| e.on && !o.left_out_eqs.contains(&e.name))
            .map(|e| e.name.clone())
            .collect()
    };
    let meets: Vec<String> = o
        .chain
        .iter()
        .zip(&o.joins)
        .filter_map(|(entry, meets)| {
            if o.filters_only || !eqs.contains(&entry.name) {
                return None;
            }
            let aim = o.aim.as_ref()?;
            let name = &entry.name;
            Some(match meets.join.as_ref()? {
                DspJoin::Matched => format!("{name} was made for {aim}: matched."),
                DspJoin::Converted { from, to } => {
                    format!("{name} was made for {to}, so the difference from {from} to {to} plays first.")
                }
                DspJoin::Refitted { to, .. } => {
                    format!("{name} was made for {to}, so the correction is fitted to {to} for it.")
                }
                DspJoin::Unknown => format!("What {name} was made against is not set."),
            })
        })
        .collect();
    if o.active.is_none() && eqs.is_empty() {
        return format!("{device} is flat: the music plays untouched.");
    }
    let mut parts = Vec::new();
    if let Some(active) = &o.active {
        parts.push(match &o.aim {
            Some(aim) => format!("corrected by {active} to {aim}"),
            None => format!("corrected by {active}"),
        });
    }
    if !eqs.is_empty() {
        parts.push(format!("through {}", joined(&eqs)));
    }
    std::iter::once(format!("Music to {device}, {}.", parts.join(", then ")))
        .chain(meets)
        .collect::<Vec<_>>()
        .join(" ")
}

const FLAT: &str = "\u{0}flat";
const UNSAVED: &str = "\u{0}unsaved";
const EDITED: &str = "\u{0}edited";

impl Render for EqPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let content = match (self.overview.clone(), self.device()) {
            (Some(o), Some(device)) => {
                let head = self.head(&o, &device, window, cx);
                let chain = self.chain(&o, &device, window, cx);
                div()
                    .flex()
                    .flex_col()
                    .gap(look.space(Space::L))
                    .child(head)
                    .child(div().h(px(290.0)).child(self.graph.clone()))
                    .child(chain)
                    .child(note(
                        sentence(&o, &self.dsp.read(cx).label(&device)),
                        Tone::Muted,
                        &look,
                    ))
                    .into_any_element()
            }
            (Some(_), None) => {
                note("No output to set EQ for.", Tone::Muted, &look).into_any_element()
            }
            _ => div().into_any_element(),
        };
        let foot = self.foot(window, cx);
        let sheets = Sheets::render(&self.sheets, window, cx);
        let prompts = prompts::render(&self.dsp, &self.sheets, window, cx);
        let dsp = self.dsp.clone();
        let device = self.device();
        div()
            .id("eq-page")
            .relative()
            .size_full()
            .bg(look.palette.bg.hsla())
            .on_drop(move |paths: &ExternalPaths, _, cx| {
                let into = device.clone().map(|device| Placement {
                    device,
                    stage: Stage::Eq,
                });
                let paths = paths.paths().to_vec();
                dsp.update(cx, |dsp, cx| dsp.import_files(paths, None, None, into, cx));
            })
            .child(
                div().id("eq-scroll").size_full().overflow_y_scroll().child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(look.space(Space::L))
                        .max_w(px(720.0))
                        .mx_auto()
                        .p(look.space(Space::Xl))
                        .child(content)
                        .child(foot),
                ),
            )
            .children(prompts)
            .children(sheets)
    }
}

impl EqPage {
    /// The device, and the preset it was set from, or Flat, or Unsaved.
    fn head(
        &mut self,
        o: &DspOverview,
        device: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let look = Look::of(window, cx);
        let dsp = self.dsp.read(cx);
        let in_use = dsp.overview.as_ref().and_then(|o| o.device.clone());
        let mut devices: Vec<String> = Vec::new();
        let mut add = |d: Option<&String>| {
            if let Some(d) = d
                && !devices.contains(d)
            {
                devices.push(d.clone());
            }
        };
        add(in_use.as_ref());
        add(Some(&device.to_owned()));
        for p in o.profiles.iter().filter(|p| !p.preset) {
            for d in &p.devices {
                add(Some(d));
            }
        }
        let mut tuned: Vec<&String> = o.tunings.keys().collect();
        tuned.sort();
        for d in tuned {
            add(Some(d));
        }
        let this = cx.weak_entity();
        let device_items = devices
            .iter()
            .map(|d| {
                let shown = if Some(d) == in_use.as_ref() {
                    format!("{} ({})", dsp.label(d), label("In use"))
                } else {
                    dsp.label(d)
                };
                let (this, d) = (this.clone(), d.clone());
                Item::choice(shown, d == device, move |_, cx| {
                    this.update(cx, |p, cx| p.show(Some(d.clone()), cx)).ok();
                })
            })
            .collect();
        let shown_device = dsp.label(device);

        let flat = self.flat();
        let presets: Vec<String> = o
            .profiles
            .iter()
            .filter(|p| p.preset)
            .map(|p| p.name.clone())
            .collect();
        let selected = match &o.preset {
            None if flat => FLAT.to_owned(),
            None => UNSAVED.to_owned(),
            Some(_) if o.preset_edited => EDITED.to_owned(),
            Some(p) => p.clone(),
        };
        let mut preset_items = vec![{
            let (dsp, device) = (self.dsp.clone(), device.to_owned());
            Item::choice(label("Flat"), selected == FLAT, move |_, cx| {
                dsp.update(cx, |d, cx| d.apply_preset(None, device.clone(), cx));
            })
        }];
        if o.preset.is_none() && !flat {
            preset_items.push(Item::choice(label("Unsaved"), true, |_, _| {}));
        }
        for name in &presets {
            if Some(name) == o.preset.as_ref() && o.preset_edited {
                preset_items.push(Item::choice(
                    format!("{name} ({})", label("edited")),
                    true,
                    |_, _| {},
                ));
            }
            let (page, name) = (cx.weak_entity(), name.clone());
            preset_items.push(Item::choice(
                name.clone(),
                selected == name,
                move |_, cx| {
                    page.update(cx, |p, cx| p.choose_preset(name.clone(), cx))
                        .ok();
                },
            ));
        }
        let preset_shown = match selected.as_str() {
            FLAT => label("Flat").to_string(),
            UNSAVED => label("Unsaved").to_string(),
            EDITED => format!(
                "{} ({})",
                o.preset.clone().unwrap_or_default(),
                label("edited")
            ),
            p => p.to_owned(),
        };

        let mut rows: Vec<AnyElement> = vec![
            FormRow::new("Device")
                .control(menu(
                    "eq-device",
                    shown_device,
                    Trigger::Field,
                    device_items,
                    window,
                    cx,
                ))
                .into_any_element(),
            FormRow::new("Preset")
                .control(menu(
                    "eq-preset",
                    preset_shown,
                    Trigger::Field,
                    preset_items,
                    window,
                    cx,
                ))
                .into_any_element(),
            {
                let (dsp, device) = (self.dsp.clone(), device.to_owned());
                Toggle::new(
                    "eq-correct",
                    "Correct for a headphone profile",
                    !o.filters_only,
                )
                .on_change(move |on, _, cx| {
                    dsp.update(cx, |d, cx| d.set_filters_only(!*on, device.clone(), cx));
                })
                .into_any_element()
            },
        ];
        if let Some(preset) = o.preset.clone().filter(|_| o.preset_edited) {
            let this = cx.weak_entity();
            let save = {
                let (this, preset) = (this.clone(), preset.clone());
                move |_: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut App| {
                    this.update(cx, |p, cx| p.save(preset.clone(), true, cx))
                        .ok();
                }
            };
            let revert = {
                let (this, preset) = (this.clone(), preset.clone());
                move |_: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut App| {
                    this.update(cx, |p, cx| p.choose_preset(preset.clone(), cx))
                        .ok();
                }
            };
            let save_new = move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut App| {
                this.update(cx, |p, cx| {
                    p.ask_preset_name("Save as New Preset", window, cx)
                })
                .ok();
            };
            rows.push(
                FormRow::data(format!("{} {preset}", label("Changed since")))
                    .control(
                        div()
                            .flex()
                            .gap(look.space(Space::M))
                            .child(
                                Button::new("preset-save", ButtonKind::Bordered)
                                    .label("Save")
                                    .on_click(save),
                            )
                            .child(
                                Button::new("preset-revert", ButtonKind::Link)
                                    .label("Revert")
                                    .on_click(revert),
                            )
                            .child(
                                Button::new("preset-new", ButtonKind::Link)
                                    .label("Save as New…")
                                    .on_click(save_new),
                            ),
                    )
                    .into_any_element(),
            );
        } else if o.preset.is_none() && !flat {
            let this = cx.weak_entity();
            rows.push(
                Button::new("preset-save-as", ButtonKind::Compact)
                    .label("Save as Preset…")
                    .on_click(move |_, window, cx| {
                        this.update(cx, |p, cx| p.ask_preset_name("Save as Preset", window, cx))
                            .ok();
                    })
                    .into_any_element(),
            );
        }
        if let Some((title, input)) = &self.naming {
            let this = cx.weak_entity();
            let cancel = this.clone();
            rows.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(look.space(Space::S))
                    .child(SectionHeader::new(title.as_ref()))
                    .child(note(
                        format!(
                            "The correction and filters, to switch {} back to, or another device to.",
                            self.dsp.read(cx).label(device)
                        ),
                        Tone::Muted,
                        &look,
                    ))
                    .child(
                        div()
                            .flex()
                            .gap(look.space(Space::M))
                            .child(div().flex_1().child(TextField::new(input)))
                            .child(Button::new("name-save", ButtonKind::Bordered).label("Save").on_click(move |_, _, cx| {
                                this.update(cx, |p, cx| p.save_named(cx)).ok();
                            }))
                            .child(Button::new("name-cancel", ButtonKind::Link).label("Cancel").on_click(move |_, _, cx| {
                                cancel
                                    .update(cx, |p, cx| {
                                        p.naming = None;
                                        p.refusal = None;
                                        cx.notify();
                                    })
                                    .ok();
                            })),
                    )
                    .into_any_element(),
            );
        }
        if let Some(refusal) = &self.refusal {
            rows.push(note(refusal.clone(), Tone::Bad, &look).into_any_element());
        }
        let sheets = self.sheets.clone();
        div()
            .flex()
            .flex_col()
            .gap(look.space(Space::Xs))
            .child(
                div()
                    .flex()
                    .items_end()
                    .justify_between()
                    .child(SectionHeader::new("Device and preset"))
                    .child(
                        Button::new("eq-explain", ButtonKind::Link)
                            .label("How EQ works")
                            .icon(Icon::Info)
                            .on_click(move |_, window, cx| {
                                let view =
                                    cx.new(|cx| stage::Explainer::new(sheets.clone(), window, cx));
                                sheets.update(cx, |s, cx| s.view(view, cx));
                            }),
                    ),
            )
            .children(rows)
            .into_any_element()
    }

    /// Choosing a preset replaces the device's correction and filters; where
    /// they hold changes no preset has, that is said first.
    fn choose_preset(&mut self, name: String, cx: &mut Context<Self>) {
        let (Some(device), Some(o)) = (self.device(), &self.overview) else {
            return;
        };
        let dsp = self.dsp.clone();
        let apply = {
            let (name, device) = (name.clone(), device.clone());
            move |_: &mut Window, cx: &mut App| {
                dsp.update(cx, |d, cx| {
                    d.apply_preset(Some(name.clone()), device.clone(), cx)
                });
            }
        };
        let unsaved = (o.preset.is_none() && !self.flat()) || o.preset_edited;
        if !unsaved {
            let dsp = self.dsp.clone();
            dsp.update(cx, |d, cx| d.apply_preset(Some(name), device, cx));
            return;
        }
        let what = match &o.preset {
            Some(p) if o.preset_edited => format!("the changes made since {p} was set"),
            _ => "a correction and filters no preset holds".to_owned(),
        };
        let label_of = self.dsp.read(cx).label(&device);
        self.sheets.update(cx, |s, cx| {
            s.confirm(
                Confirm {
                    title: format!("Set {label_of} from {name}?").into(),
                    message: format!(
                        "{label_of} plays {what}. Setting it from {name} replaces them; save them as a preset first to keep them."
                    )
                    .into(),
                    action: "Set".into(),
                    destructive: true,
                    also: None,
                    run: Rc::new(apply),
                },
                cx,
            )
        });
    }

    /// The suggestion, what the last import did, the last error, and Manage EQ.
    fn foot(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let look = Look::of(window, cx);
        let dsp = self.dsp.read(cx);
        let mut rows: Vec<AnyElement> = Vec::new();
        let on_output_in_use =
            self.device() == dsp.overview.as_ref().and_then(|o| o.device.clone());
        let (suggestion, summary, error) = (
            dsp.suggestion.clone(),
            dsp.import_summary.clone(),
            dsp.last_error.clone(),
        );
        if let Some(offer) = suggestion
            && on_output_in_use
            && self.overview.as_ref().is_some_and(|o| !o.filters_only)
        {
            rows.push(flows::suggestion(
                offer,
                self.dsp.clone(),
                self.sheets.clone(),
                &look,
                window,
                cx,
            ));
        }
        if let Some(summary) = summary {
            rows.push(note(summary, Tone::Muted, &look).into_any_element());
        }
        if let Some(error) = error {
            rows.push(note(error.clone(), Tone::Bad, &look).into_any_element());
        }
        let (dsp, sheets) = (self.dsp.clone(), self.sheets.clone());
        let device = self.device();
        rows.push(
            Button::new("manage-eq", ButtonKind::Link)
                .label("Manage EQ")
                .on_click(move |_, window, cx| {
                    let view = cx.new(|cx| {
                        manage::ManageEq::new(
                            dsp.clone(),
                            sheets.clone(),
                            device.clone(),
                            window,
                            cx,
                        )
                    });
                    sheets.update(cx, |s, cx| s.view(view, cx));
                })
                .into_any_element(),
        );
        div()
            .flex()
            .flex_col()
            .gap(look.space(Space::S))
            .children(rows)
            .into_any_element()
    }

    // MARK: - The chain

    fn chain(
        &mut self,
        o: &DspOverview,
        device: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let look = Look::of(window, cx);
        let correction = o
            .profiles
            .iter()
            .find(|p| Some(&p.name) == o.active.as_ref())
            .cloned();
        let matched = o.matched;
        let outline = if matched {
            look.accent.hsla()
        } else {
            look.palette.rule.hsla()
        };
        let link = |look: &Look| {
            div()
                .ml(px(18.0))
                .w(if matched { MARK } else { HAIRLINE })
                .h(px(14.0))
                .bg(if matched {
                    look.accent.hsla()
                } else {
                    look.palette.rule.hsla()
                })
        };
        let end = |title: SharedString, icon: Icon, look: &Look| {
            div()
                .flex()
                .items_center()
                .gap(look.space(Space::S))
                .pl(px(8.0))
                .text_role(TypeRole::Meta, look)
                .text_color(look.text_color(Tone::Muted))
                .when(look.icons, |d| {
                    d.child(KoanIcon::new(icon).role(TypeRole::Meta).tone(Tone::Muted))
                })
                .child(title)
        };
        let mut col: Vec<AnyElement> = vec![
            end(label("Music in"), Icon::Track, &look).into_any_element(),
            link(&look).into_any_element(),
        ];
        let pick = |stage: Stage, page: &Self| {
            let (dsp, sheets, device) = (page.dsp.clone(), page.sheets.clone(), device.to_owned());
            move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut App| {
                let view = cx.new(|cx| {
                    stage::StagePicker::new(
                        dsp.clone(),
                        sheets.clone(),
                        stage,
                        device.clone(),
                        window,
                        cx,
                    )
                });
                sheets.update(cx, |s, cx| s.view(view, cx));
            }
        };
        if !o.filters_only {
            match &correction {
                Some(c) => {
                    let detail = if c.role == DspRole::Baked {
                        label("Already includes a tuning").to_string()
                    } else {
                        [
                            Some(describe_profile(c)),
                            o.aim.as_ref().map(|a| format!("to {a}")),
                        ]
                        .into_iter()
                        .flatten()
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join(" · ")
                    };
                    let open = self.opener(c.name.clone());
                    col.push(
                        block(
                            "stage-correction",
                            "Correction",
                            c.name.clone(),
                            Some(detail),
                            self.curves.get(&c.name).cloned(),
                            Stroke::CORRECTION,
                            outline,
                            &look,
                            pick(Stage::Correction, self),
                            vec![
                                Button::new("show-correction", ButtonKind::Link)
                                    .label("Show")
                                    .on_click(open)
                                    .into_any_element(),
                            ],
                        )
                        .into_any_element(),
                    );
                }
                None => col.push(
                    placeholder(
                        "add-correction",
                        Some("Correction"),
                        "Add a correction",
                        None,
                        &look,
                        pick(Stage::Correction, self),
                    )
                    .into_any_element(),
                ),
            }
            col.push(link(&look).into_any_element());
        }
        col.push(
            div()
                .py(px(4.0))
                .text_role(TypeRole::Fine, &look)
                .text_color(look.text_color(Tone::Muted))
                .child(label("Filters"))
                .into_any_element(),
        );
        for (i, entry) in o.chain.iter().enumerate() {
            let meets = o.joins.get(i);
            if o.filters_only {
                if i > 0 {
                    col.push(link(&look).into_any_element());
                }
            } else if i > 0 || meets.is_some_and(|m| m.join.is_some() || m.note.is_some()) {
                col.push(self.join(meets, &entry.name, matched, &look, cx));
            }
            let detail = if entry.on {
                if o.filters_only {
                    None
                } else {
                    meets
                        .and_then(|m| m.made_for.clone())
                        .map(|m| format!("made for {m}"))
                }
            } else {
                Some(label("Off").to_string())
            };
            col.push(self.eq_block(i, entry, o, device, detail, outline, window, cx));
        }
        if !o.chain.is_empty() {
            col.push(link(&look).into_any_element());
        }
        if correction.as_ref().is_none_or(|c| c.role != DspRole::Baked) {
            col.push(
                placeholder(
                    "add-eq",
                    None,
                    "Add EQ",
                    matched.then_some(outline),
                    &look,
                    pick(Stage::Eq, self),
                )
                .into_any_element(),
            );
            col.push(link(&look).into_any_element());
        }
        col.push(
            end(
                format!("{} {}", self.dsp.read(cx).label(device), label("out")).into(),
                Icon::Speaker,
                &look,
            )
            .into_any_element(),
        );
        div().flex().flex_col().children(col).into_any_element()
    }

    fn opener(
        &self,
        name: String,
    ) -> impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static {
        let (dsp, sheets, device) = (self.dsp.clone(), self.sheets.clone(), self.device());
        move |_, window, cx| {
            let view = cx.new(|cx| {
                super::profile::ProfilePage::new(
                    dsp.clone(),
                    sheets.clone(),
                    name.clone(),
                    device.clone(),
                    window,
                    cx,
                )
            });
            sheets.update(cx, |s, cx| s.view(view, cx));
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn eq_block(
        &self,
        i: usize,
        entry: &DspTuningEntry,
        o: &DspOverview,
        device: &str,
        detail: Option<String>,
        outline: gpui_kit::Hsla,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let look = Look::of(window, cx);
        let set = {
            let (dsp, device) = (self.dsp.clone(), device.to_owned());
            move |chain: Vec<DspTuningEntry>, cx: &mut App| {
                dsp.update(cx, |d, cx| d.set_tunings(chain, device.clone(), cx));
            }
        };
        let set = Rc::new(set);
        let chain = o.chain.clone();
        let toggle = {
            let (set, chain) = (set.clone(), chain.clone());
            Toggle::new(("eq-on", i), "On", entry.on).on_change(move |on, _, cx| {
                let mut c = chain.clone();
                c[i].on = *on;
                set(c, cx);
            })
        };
        let mut items = vec![Item::action(label("Edit"), {
            let open = self.opener(entry.name.clone());
            move |window, cx| open(&gpui_kit::ClickEvent::default(), window, cx)
        })];
        if i > 0 {
            let (set, chain) = (set.clone(), chain.clone());
            items.push(Item::action(label("Move Up"), move |_, cx| {
                let mut c = chain.clone();
                c.swap(i, i - 1);
                set(c, cx);
            }));
        }
        if i + 1 < chain.len() {
            let (set, chain) = (set.clone(), chain.clone());
            items.push(Item::action(label("Move Down"), move |_, cx| {
                let mut c = chain.clone();
                c.swap(i, i + 1);
                set(c, cx);
            }));
        }
        {
            let (set, chain) = (set.clone(), chain.clone());
            items.push(Item::action(label("Remove"), move |_, cx| {
                let mut c = chain.clone();
                c.remove(i);
                set(c, cx);
            }));
        }
        let options = menu(
            ("eq-options", i),
            label("Options"),
            Trigger::Word,
            items,
            window,
            cx,
        );
        block(
            ("stage-eq", i),
            &format!("EQ {}", i + 1),
            entry.name.clone(),
            detail,
            self.curves.get(&entry.name).cloned(),
            Stroke::eq(i),
            outline,
            &look,
            self.opener(entry.name.clone()),
            vec![toggle.into_any_element(), options],
        )
        .into_any_element()
    }

    /// The line into an EQ, saying how it meets the correction, and what of
    /// it does not play as chosen.
    fn join(
        &self,
        meets: Option<&DspEqJoin>,
        eq: &str,
        chain_matched: bool,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let matched =
            chain_matched || meets.is_some_and(|m| matches!(m.join, Some(DspJoin::Matched)));
        let mut lines: Vec<AnyElement> = Vec::new();
        match meets.and_then(|m| m.join.clone()) {
            Some(DspJoin::Matched) => lines.push(
                div()
                    .flex()
                    .items_center()
                    .gap(look.space(Space::Xs))
                    .text_role(TypeRole::Fine, look)
                    .text_color(look.text_color(Tone::Accent))
                    .when(look.icons, |d| {
                        d.child(
                            KoanIcon::new(Icon::Check)
                                .role(TypeRole::Fine)
                                .tone(Tone::Accent),
                        )
                    })
                    .child(label("Matched"))
                    .into_any_element(),
            ),
            Some(DspJoin::Converted { from, to }) => {
                let step = meets.map(|m| m.step.clone()).unwrap_or_default();
                lines.push(
                    div()
                        .flex()
                        .items_center()
                        .gap(look.space(Space::M))
                        .child(note(
                            format!("Target difference: {from} → {to}"),
                            Tone::Muted,
                            look,
                        ))
                        .when(step.len() > 1, |d| {
                            d.child(thumb(step, Stroke::solid_muted(), None))
                        })
                        .into_any_element(),
                );
            }
            Some(DspJoin::Refitted { to, .. }) => lines.push(
                note(
                    format!("Correction fitted to {to} for it"),
                    Tone::Muted,
                    look,
                )
                .into_any_element(),
            ),
            Some(DspJoin::Unknown) => {
                let open = self.opener(eq.to_owned());
                lines.push(
                    div()
                        .id(SharedString::from(format!("join-unknown-{eq}")))
                        .cursor_pointer()
                        .on_click(open)
                        .child(note("Made against: not set. Set it", Tone::Muted, look))
                        .into_any_element(),
                );
                if let Some(s) = meets.and_then(|m| m.suggestion.clone()) {
                    let (dsp, eq) = (self.dsp.clone(), eq.to_owned());
                    lines.push(
                        Button::new(
                            SharedString::from(format!("join-suggest-{eq}")),
                            ButtonKind::Link,
                        )
                        .label(format!("Looks made for {}. Use that?", s.name))
                        .on_click(move |_, _, cx| {
                            let (eq, id) = (eq.clone(), s.id.clone());
                            dsp.update(cx, |d, cx| {
                                d.act(cx, move |e| e.dsp_set_tuned_for(eq, Some(id)))
                            });
                        })
                        .into_any_element(),
                    );
                }
            }
            None => {}
        }
        if let Some(n) = meets.and_then(|m| m.note.clone()) {
            lines.push(
                div()
                    .flex()
                    .items_center()
                    .gap(look.space(Space::Xs))
                    .when(look.icons, |d| {
                        d.child(
                            KoanIcon::new(Icon::Warning)
                                .role(TypeRole::Fine)
                                .tone(Tone::Bad),
                        )
                    })
                    .child(note(n, Tone::Bad, look))
                    .into_any_element(),
            );
        }
        let _ = cx;
        div()
            .relative()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .py(px(4.0))
            .min_h(px(14.0))
            .pl(px(18.0) + look.space(Space::M))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(18.0))
                    .w(if matched { MARK } else { HAIRLINE })
                    .bg(if matched {
                        look.accent.hsla()
                    } else {
                        look.palette.rule.hsla()
                    }),
            )
            .children(lines)
            .into_any_element()
    }
}

impl Stroke {
    fn solid_muted() -> Stroke {
        Stroke {
            tone: Tone::Muted,
            opacity: 1.0,
            width: 1.5,
            dash: &[],
        }
    }
}

/// A stage of the chain: what it is, what fills it, its curve, and its
/// controls.
#[allow(clippy::too_many_arguments)]
fn block(
    id: impl Into<gpui_kit::ElementId>,
    title: &str,
    name: String,
    detail: Option<String>,
    db: Option<Vec<f64>>,
    stroke: Stroke,
    outline: gpui_kit::Hsla,
    look: &Look,
    on_click: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static,
    controls: Vec<AnyElement>,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(look.space(Space::M))
        .p(look.space(Space::S))
        .border(HAIRLINE)
        .border_color(outline)
        .child(
            div()
                .id(id.into())
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap(look.space(Space::M))
                .cursor_pointer()
                .on_click(on_click)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(note(label(title), Tone::Muted, look))
                        .child(
                            div()
                                .text_role(TypeRole::Body, look)
                                .text_color(look.palette.ink.hsla())
                                .truncate()
                                .child(name),
                        )
                        .children(
                            detail
                                .filter(|d| !d.is_empty())
                                .map(|d| note(d, Tone::Muted, look).truncate()),
                        ),
                )
                .children(db.map(|db| thumb(db, stroke, None))),
        )
        .children(controls)
}

/// An empty stage: a dashed place to choose what fills it.
fn placeholder(
    id: &'static str,
    title: Option<&str>,
    prompt: &str,
    outline: Option<gpui_kit::Hsla>,
    look: &Look,
    on_click: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex()
        .flex_col()
        .gap(px(2.0))
        .p(look.space(Space::S))
        .border(HAIRLINE)
        .border_dashed()
        .border_color(outline.unwrap_or(look.palette.muted.hsla()))
        .cursor_pointer()
        .on_click(on_click)
        .children(title.map(|t| note(label(t), Tone::Muted, look)))
        .child(
            div()
                .flex()
                .items_center()
                .gap(look.space(Space::Xs))
                .text_role(TypeRole::Body, look)
                .text_color(look.text_color(Tone::Accent))
                .when(look.icons, |d| {
                    d.child(
                        KoanIcon::new(Icon::Add)
                            .role(TypeRole::Body)
                            .tone(Tone::Accent),
                    )
                })
                .child(label(prompt)),
        )
}

/// A curve as a small line, for a stage's block: 64 × 26, scaled to the
/// curve's own peak (at least 6 dB) unless `range` shares one.
pub fn thumb(db: Vec<f64>, stroke: Stroke, range: Option<f64>) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, cx| {
            if db.len() < 2 {
                return;
            }
            let look = Look::of(window, cx);
            let range = range.unwrap_or_else(|| db.iter().fold(6.0f64, |m, v| m.max(v.abs())));
            let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
            let mut b = gpui_kit::PathBuilder::stroke(px(1.5));
            if !stroke.dash.is_empty() {
                let dash: Vec<_> = stroke.dash.iter().map(|d| px(*d)).collect();
                b = b.dash_array(&dash);
            }
            let n = db.len();
            for (i, v) in db.iter().enumerate() {
                let p = bounds.origin
                    + point(
                        px(w * i as f32 / (n - 1) as f32),
                        px(h / 2.0 - h / 2.0 * (*v / range) as f32),
                    );
                if i == 0 {
                    b.move_to(p);
                } else {
                    b.line_to(p);
                }
            }
            if let Ok(path) = b.build() {
                window.paint_path(path, stroke.color(&look));
            }
        },
    )
    .w(px(64.0))
    .h(px(26.0))
    .flex_none()
}

/// What an offer of AutoEQ's profile says. Kept here so the page can test
/// it without a window.
pub fn offer_line(offer: &AutoEqOffer) -> String {
    match offer {
        AutoEqOffer::Profile { entry } => {
            format!("AutoEQ has a correction for {}. Use it?", entry.name)
        }
        AutoEqOffer::Search { query } => format!("Find {query} in AutoEQ…"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use koan_ffi::{DspProfileSummary, DspTuningEntry};

    fn overview(active: Option<&str>, chain: &[(&str, bool)]) -> DspOverview {
        DspOverview {
            enabled: true,
            device: Some("DAC".into()),
            active: active.map(Into::into),
            tuning: None,
            tuning_plays: true,
            left_out: None,
            left_out_eqs: Vec::new(),
            outputs: Vec::new(),
            tunings: Default::default(),
            chain: chain
                .iter()
                .map(|(n, on)| DspTuningEntry {
                    name: (*n).into(),
                    on: *on,
                })
                .collect(),
            joins: Vec::new(),
            aim: None,
            in_ear: None,
            preset: None,
            preset_edited: false,
            profiles: Vec::<DspProfileSummary>::new(),
            names: Default::default(),
            filters_only: false,
            matched: false,
        }
    }

    #[test]
    fn the_sentence_says_what_plays() {
        assert_eq!(
            sentence(&overview(None, &[]), "DAC"),
            "DAC is flat: the music plays untouched."
        );
        assert_eq!(
            sentence(
                &overview(
                    Some("HD 600"),
                    &[("Warm", true), ("Off", false), ("Tilt", true)]
                ),
                "DAC"
            ),
            "Music to DAC, corrected by HD 600, then through Warm and Tilt."
        );
    }
}
