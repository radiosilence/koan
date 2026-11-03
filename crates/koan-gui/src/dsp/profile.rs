//! One profile, and exactly what is in it: its graph, which edits a tuning,
//! each impulse response's rate, channels, length and routing, its bands,
//! the headroom it is given, and the outputs that play through it. Renamed,
//! copied, put back and deleted from here.

use std::collections::HashMap;
use std::rc::Rc;

use koan_kit::app::input::{InputEvent, InputState};
use koan_kit::app::*;
use koan_runtime::{
    DspDeviceAction, DspEarKind, DspLayerInfo, DspOverview, DspProfileDetail, DspRole,
    DspTargetName, DspTargetOption, DspTargets, DspTuningEntry,
};

use super::bands::{self, Fields};
use super::graph::{Editing, Graph, Part, Stroke};
use super::menu::{Item, Trigger, menu};
use super::model::{Dsp, GRAPH_RATE, SYNC_EXTENSION, joined, khz};
use super::page::thumb;
use super::sheet::{Confirm, Sheets, body, head, head_data, note, section};
use super::stage::target_label;
use crate::components::{
    Badge, Button, ButtonKind, FormRow, SectionHeader, Segmented, TextField, Toggle,
};
use crate::theme::{Icon, KoanIcon, KoanStyled, Look, Space, Tone, TypeRole, label};

/// A profile's sheet: its name and Done, the graph, which stays in view
/// while the bands scroll under it, and the body, drawn by `ProfileBody` as
/// a cached view so that a drag on the graph redraws the graph and this
/// frame and nothing of the body.
pub struct ProfilePage {
    body: Entity<ProfileBody>,
    graph: Entity<Graph>,
    sheets: Entity<Sheets>,
    _subscriptions: Vec<Subscription>,
}

impl ProfilePage {
    pub fn new(
        dsp: Entity<Dsp>,
        sheets: Entity<Sheets>,
        name: String,
        device: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let body = cx.new(|cx| ProfileBody::new(dsp, sheets.clone(), name, device, window, cx));
        let graph = body.read(cx).graph.clone();
        let subscriptions = vec![cx.observe(&body, |_, _, cx| cx.notify())];
        Self {
            body,
            graph,
            sheets,
            _subscriptions: subscriptions,
        }
    }

    pub fn graph(&self) -> Entity<Graph> {
        self.graph.clone()
    }
}

impl Render for ProfilePage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let (name, curve) = self.body.read(cx).shown();
        let head = if curve {
            let body = self.body.clone();
            let back = Button::new("curve-back", ButtonKind::Standard)
                .label("Back")
                .on_click(move |_, _, cx| {
                    body.update(cx, |p, cx| {
                        p.curve = None;
                        cx.notify();
                    })
                });
            head("Graphic EQ", vec![back.into_any_element()], &look).into_any_element()
        } else {
            let (sheets, me) = (self.sheets.clone(), cx.entity_id());
            let done = Button::new("profile-done", ButtonKind::Standard)
                .label("Done")
                .on_click(move |_, _, cx| sheets.update(cx, |s, cx| s.close(me, cx)));
            head_data(name, vec![done.into_any_element()], &look).into_any_element()
        };
        // A definite height, so the cached body has one to lay out in.
        let height = window.viewport_size().height * 0.8;
        div()
            .flex()
            .flex_col()
            .w_full()
            .h(height)
            .child(head)
            .when(!curve, |d| {
                d.child(
                    div()
                        .px(look.space(Space::L))
                        .py(look.space(Space::M))
                        .border_b(look.hairline)
                        .border_color(look.palette.rule.hsla())
                        .child(
                            div().h(Pixels::from(GRAPH_H)).child(
                                koan_kit::app::AnyView::from(self.graph.clone())
                                    // the bounds a cached view keeps between frames.
                                    .cached(StyleRefinement::default().size_full()),
                            ),
                        ),
                )
            })
            .child(koan_kit::app::AnyView::from(self.body.clone()).cached(
                // the bounds a cached view keeps between frames.
                StyleRefinement::default().flex_1().min_h_0().w_full(),
            ))
    }
}

/// The graph's room at the top of the page: the chart, its legend and the
/// brushes, held to one height so the graph can be cached.
const GRAPH_H: f32 = 290.0;
/// The curve drawn inside that frame, in points.
const CURVE_H: f32 = 220.0;

pub struct ProfileBody {
    dsp: Entity<Dsp>,
    sheets: Entity<Sheets>,
    name: String,
    /// The output Used for acts on, and whose filters a copy saved here
    /// takes this EQ's place in; the one in use if none.
    device: Option<String>,
    detail: Option<DspProfileDetail>,
    targets: Option<DspTargets>,
    device_overview: Option<DspOverview>,
    /// Every target by ear, for what a ready-made EQ was made for.
    over: Vec<DspTargetOption>,
    in_ear: Vec<DspTargetOption>,
    /// For a tuning: the target it looks made against while it does not
    /// say, and what it adds on the output's correction for each choice.
    suggestion: Option<DspTargetName>,
    previews: HashMap<String, Vec<f64>>,
    /// The EQ as it was when the page opened, which Save as Copy puts back,
    /// and as it is now; and the edits made here when `before` was taken.
    before: Option<String>,
    now: Option<String>,
    edits_at_before: Option<u64>,
    notice: Option<SharedString>,
    channel: u16,
    more: bool,
    graph: Entity<Graph>,
    fields: Fields,
    /// The graphic curve whose points are shown, by its index.
    curve: Option<usize>,
    name_input: Entity<InputState>,
    /// The window the page is in, for its fields.
    window: koan_kit::app::AnyWindowHandle,
    copy_input: Option<Entity<InputState>>,
    loaded: Option<((u64, u64), String)>,
    /// The stamp the detail shown was read at, as its read started.
    read: Option<(u64, u64)>,
    _load: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl ProfileBody {
    pub fn new(
        dsp: Entity<Dsp>,
        sheets: Entity<Sheets>,
        name: String,
        device: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let graph = cx
            .new(|cx| Graph::new(Graph::flat_response(), window, cx).height(Pixels::from(CURVE_H)));
        let name_input = cx.new(|cx| InputState::new(window, cx).default_value(name.clone()));
        let subscriptions = vec![
            cx.observe(&dsp, |p: &mut Self, _, cx| p.load(cx)),
            cx.subscribe_in(
                &name_input,
                window,
                |p: &mut Self, _, e: &InputEvent, _, cx| {
                    // Renamed on Return; left without it, the field shows the
                    // name again.
                    match e {
                        InputEvent::PressEnter { .. } => p.rename(cx),
                        InputEvent::Blur => p.show_name(cx),
                        _ => {}
                    }
                },
            ),
        ];
        let mut page = Self {
            fields: Fields::new(dsp.clone(), name.clone()),
            dsp,
            sheets,
            name,
            device,
            detail: None,
            targets: None,
            device_overview: None,
            over: Vec::new(),
            in_ear: Vec::new(),
            suggestion: None,
            previews: HashMap::new(),
            before: None,
            now: None,
            edits_at_before: None,
            notice: None,
            channel: 0,
            more: false,
            graph,
            curve: None,
            name_input,
            window: window.window_handle(),
            copy_input: None,
            loaded: None,
            read: None,
            _load: None,
            _subscriptions: subscriptions,
        };
        page.load(cx);
        page
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let stamp = self.dsp.read(cx).stamp();
        let key = (stamp, self.name.clone());
        if self.loaded.as_ref() == Some(&key) {
            cx.notify();
            return;
        }
        self.loaded = Some(key);
        let engine = self.dsp.read(cx).engine();
        let (name, device) = (self.name.clone(), self.device.clone());
        self._load = Some(cx.spawn(async move |this, cx| {
            let e = || engine.clone();
            let snapshot = e().dsp_snapshot(name.clone()).await;
            // Counted once the snapshot is in: an edit made since is in
            // neither, one made before is in both.
            let Ok(edits) = this.read_with(cx, |p, cx| p.dsp.read(cx).edits(&name)) else {
                return;
            };
            let detail = e().dsp_detail(name.clone()).await;
            let response = e().dsp_response(name.clone(), GRAPH_RATE).await;
            let targets = e().dsp_targets(name.clone()).await;
            let device_overview = match &device {
                Some(d) => Some(e().dsp_overview_for(Some(d.clone())).await),
                None => None,
            };
            let over = e().dsp_targets_for(DspEarKind::OverEar).await;
            let in_ear = e().dsp_targets_for(DspEarKind::InEar).await;
            let tuning = detail.as_ref().is_some_and(|d| d.role == DspRole::Tuning);
            let (suggestion, previews) = if tuning {
                (
                    e().dsp_suggest_made_against(name.clone(), device.clone())
                        .await,
                    e().dsp_made_against_previews(name.clone(), device.clone())
                        .await
                        .into_iter()
                        .map(|p| (p.target.unwrap_or_default(), p.db))
                        .collect(),
                )
            } else {
                (None, HashMap::new())
            };
            this.update(cx, |p, cx| {
                if p.name != name {
                    return;
                }
                p.now = snapshot.clone();
                // Until an edit is made here, a change is another device's
                // and what Save as Copy puts back follows it.
                if p.before.is_none() || p.edits_at_before == Some(edits) {
                    p.before = snapshot;
                    p.edits_at_before = Some(edits);
                }
                p.targets = targets;
                p.device_overview = device_overview;
                p.over = over;
                p.in_ear = in_ear;
                p.suggestion = suggestion;
                p.previews = previews;
                if let Some(d) = &detail {
                    p.fields.keep(&d.bands);
                }
                p.detail = detail;
                p.read = Some(stamp);
                p.notice = None;
                p.show_name(cx);
                p.sync_graph(response, stamp, cx);
                cx.notify();
            })
            .ok();
        }));
    }

    /// Puts the profile's name back in its field, unless it is being typed
    /// in.
    fn show_name(&mut self, cx: &mut Context<Self>) {
        let name = self.name.clone();
        let input = self.name_input.clone();
        self.window
            .update(cx, |_, window, cx| {
                if input.read(cx).value() != name.as_str() {
                    input.update(cx, |i, cx| i.set_value(name, window, cx));
                }
            })
            .ok();
    }

    /// The stamp the filters shown were read at.
    fn read_at(&self) -> (u64, u64) {
        self.read.unwrap_or_default()
    }

    pub fn graph(&self) -> Entity<Graph> {
        self.graph.clone()
    }

    fn stereo_channel(&self) -> Option<u16> {
        self.detail
            .as_ref()
            .and_then(|d| d.channels.as_ref())
            .filter(|c| c.stereo)
            .map(|_| self.channel)
    }

    fn sync_graph(
        &mut self,
        response: Option<koan_runtime::DspResponse>,
        stamp: (u64, u64),
        cx: &mut Context<Self>,
    ) {
        let Some(d) = self.detail.clone() else { return };
        let editing = (!d.read_only).then(|| Editing {
            dsp: self.dsp.clone(),
            name: d.name.clone(),
            bands: d.bands.clone(),
            channel: self.stereo_channel().unwrap_or(0),
        });
        let parts = self.on_correction(&d, response.as_ref());
        self.graph.update(cx, |g, cx| {
            if let Some(r) = response {
                g.set_response(r, stamp, cx);
            }
            g.set_parts(parts, cx);
            g.set_editing(editing, cx);
        });
    }

    /// A tuning as it plays on the output's correction, where that differs
    /// from the tuning alone: with the target difference its Made against
    /// asks for, so a wrong choice shows as a preference doubled or taken
    /// out.
    fn on_correction(
        &self,
        d: &DspProfileDetail,
        r: Option<&koan_runtime::DspResponse>,
    ) -> Vec<Part> {
        let (Some(r), Some(db)) = (r, self.previews.get(d.tuned_for.as_deref().unwrap_or("")))
        else {
            return Vec::new();
        };
        if db.len() != r.total.len() || db == &r.total {
            return Vec::new();
        }
        vec![Part {
            name: "On the correction in use".into(),
            db: db.clone(),
            stroke: Stroke::eq(1),
        }]
    }

    /// The name shown, and whether a curve's points are.
    fn shown(&self) -> (String, bool) {
        let curve = self.curve.is_some_and(|i| {
            self.detail
                .as_ref()
                .is_some_and(|d| d.bands.get(i).is_some_and(|b| b.kind == "graphic"))
        });
        (self.name.clone(), curve)
    }

    fn confirm(&self, confirm: Confirm, cx: &mut App) {
        self.sheets.update(cx, |s, cx| s.confirm(confirm, cx));
    }

    /// Renames once the name is left or returned; one in use is refused and
    /// the field put back. Every device and preset naming it follows, which
    /// is said first.
    fn rename(&mut self, cx: &mut Context<Self>) {
        let new = self.name_input.read(cx).value().trim().to_owned();
        if new.is_empty() || new == self.name {
            return;
        }
        let old = self.name.clone();
        let this = cx.weak_entity();
        let dsp = self.dsp.clone();
        let mut lines = vec![format!("{old} is renamed {new} everywhere it is used.")];
        if let Some(p) = dsp.read(cx).profile(&old) {
            if !p.used_on.is_empty() {
                lines.push(format!(
                    "The outputs that play it, {}, play it as {new}.",
                    joined(&p.used_on)
                ));
            }
            if !p.held_by.is_empty() {
                lines.push(format!("{} hold it as {new}.", joined(&p.held_by)));
            }
        }
        let (from, to) = (old.clone(), new.clone());
        self.confirm(
            Confirm {
                title: format!("Rename {old}?").into(),
                message: lines.join(" ").into(),
                action: "Rename".into(),
                destructive: false,
                also: None,
                run: Rc::new(move |_, cx| {
                    let (from, to, this) = (from.clone(), to.clone(), this.clone());
                    let renamed = to.clone();
                    dsp.update(cx, |d, cx| {
                        d.act_then(
                            cx,
                            move |e| e.dsp_rename(from, to),
                            move |taken, cx| {
                                this.update(cx, |p, cx| {
                                    if taken.is_some() {
                                        p.name = renamed.clone();
                                        p.fields.rename(renamed.clone());
                                        p.loaded = None;
                                        p.load(cx);
                                    } else {
                                        p.show_name(cx);
                                    }
                                })
                                .ok();
                            },
                        )
                    });
                }),
            },
            cx,
        );
    }

    /// The device Used for acts on, and what it plays.
    fn target_device(&self, cx: &App) -> Option<(String, DspOverview)> {
        match &self.device {
            Some(d) => Some((d.clone(), self.device_overview.clone()?)),
            None => {
                let o = self.dsp.read(cx).overview.clone()?;
                Some((o.device.clone()?, o))
            }
        }
    }

    fn device_action(&self, d: &DspProfileDetail, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (device, shown) = self.target_device(cx)?;
        let in_chain = shown.chain.iter().any(|e| e.name == d.name);
        let action =
            koan_runtime::dsp_device_action(d.name.clone(), d.role, shown.active.clone(), in_chain);
        let words = match action {
            DspDeviceAction::StopCorrecting => "Stop using for",
            DspDeviceAction::MoveToFilters => "Play it in the filters instead on",
            DspDeviceAction::TakeOutOfFilters => "Take out of the filters on",
            DspDeviceAction::AddToFilters => "Add to the filters on",
            DspDeviceAction::Correct => "Use for",
        };
        let chain = shown.chain.clone();
        let mut added = chain.clone();
        if !in_chain {
            added.push(DspTuningEntry {
                name: d.name.clone(),
                on: true,
            });
        }
        let (dsp, name) = (self.dsp.clone(), d.name.clone());
        let device_label = self.dsp.read(cx).label(&device);
        Some(
            Button::new("used-for", ButtonKind::Bordered)
                .text(format!("{} {device_label}", label(words)))
                .on_click(move |_, _, cx| {
                    let (device, chain, added, name) =
                        (device.clone(), chain.clone(), added.clone(), name.clone());
                    dsp.update(cx, |dsp, cx| match action {
                        DspDeviceAction::StopCorrecting => dsp.assign(None, device, cx),
                        DspDeviceAction::MoveToFilters => dsp.uncorrect(device, added, cx),
                        DspDeviceAction::TakeOutOfFilters => dsp.set_tunings(
                            chain.into_iter().filter(|e| e.name != name).collect(),
                            device,
                            cx,
                        ),
                        DspDeviceAction::AddToFilters => dsp.set_tunings(added, device, cx),
                        DspDeviceAction::Correct => dsp.assign(Some(name), device, cx),
                    });
                })
                .into_any_element(),
        )
    }

    fn save_copy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.copy_input.take() else {
            return;
        };
        let new = input.read(cx).value().trim().to_owned();
        let taken = self.dsp.read(cx).profile(&new).is_some();
        if taken {
            self.copy_input = Some(input);
            self.notice = Some(format!("There is already an EQ called {new}.").into());
            cx.notify();
            return;
        }
        let _ = window;
        let (from, kept) = (self.name.clone(), self.before.clone());
        let on = self.device.clone().or_else(|| {
            self.dsp
                .read(cx)
                .overview
                .as_ref()
                .and_then(|o| o.device.clone())
        });
        let on_label = on
            .as_ref()
            .map(|d| self.dsp.read(cx).label(d))
            .unwrap_or_else(|| "the output".into());
        let this = cx.weak_entity();
        self.dsp.update(cx, |d, cx| {
            let from2 = from.clone();
            d.act_then(
                cx,
                move |e| async move { e.dsp_save_as_copy(from2, (!new.is_empty()).then_some(new), kept, on).await },
                move |copy, cx| {
                    let Some(copy) = copy else { return };
                    this.update(cx, |p, cx| {
                        p.notice = (!copy.placed).then(|| {
                            format!(
                                "{} is saved, but {from} is not in {on_label}'s filters, so the copy is not either. Add it on the EQ page.",
                                copy.name
                            )
                            .into()
                        });
                        p.before = None;
                        p.name = copy.name.clone();
                        p.fields.rename(copy.name.clone());
                        p.loaded = None;
                        p.load(cx);
                    })
                    .ok();
                },
            )
        });
        cx.notify();
    }

    fn add_target(&self, cx: &mut App) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        let dsp = self.dsp.clone();
        cx.spawn(async move |cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            dsp.update(cx, |d, cx| {
                d.act(cx, move |e| async move {
                    e.dsp_add_target(path.display().to_string())
                        .await
                        .map(|_| ())
                })
            });
        })
        .detach();
    }
}

impl Render for ProfileBody {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let this = cx.weak_entity();
        let Some(d) = self.detail.clone() else {
            return div().into_any_element();
        };
        if let Some(index) = self.curve {
            if d.bands.get(index).is_some_and(|b| b.kind == "graphic") {
                return self.curve_page(index, &d, window, cx);
            }
            // The curve is gone, removed elsewhere: back to the profile.
            self.curve = None;
        }
        let mut s: Vec<AnyElement> = Vec::new();
        s.push(summary(&d, &look));
        if let Some(twice) = &d.corrects_twice {
            s.push(warning(twice.clone(), &look));
        }
        s.push(TextField::new(&self.name_input).into_any_element());
        if let Some(message) = self
            .notice
            .clone()
            .or_else(|| self.dsp.read(cx).last_error.clone())
        {
            s.push(warning(message, &look));
        }
        if let Some(layout) = d.channels.clone().filter(|_| !d.read_only) {
            s.push(self.channels(&d, layout, &look, cx));
        }
        if !d.read_only && (self.before != self.now || (d.can_revert && d.edited)) {
            s.push(self.keeping(&d, &look, window, cx));
        }
        if let Some(problem) = &d.problem {
            s.push(warning(problem.clone(), &look));
        }
        {
            let mut rows: Vec<AnyElement> = Vec::new();
            if d.devices.is_empty() {
                rows.push(note(label("No output yet"), Tone::Muted, &look).into_any_element());
            }
            for device in &d.devices {
                rows.push(
                    div()
                        .text_role(TypeRole::Body, &look)
                        .text_color(look.palette.ink.hsla())
                        .child(self.dsp.read(cx).label(device))
                        .into_any_element(),
                );
            }
            rows.extend(self.device_action(&d, cx));
            s.push(section(Some("Used for"), rows, None, &look).into_any_element());
        }
        if d.layers.is_empty() || !d.bands.is_empty() || !d.impulses.is_empty() {
            s.push(self.role_section(&d, window, cx));
        }
        if d.group {
            s.push(self.group_section(&d, window, cx));
        } else if let Some(layers) = self.layers_section(&d, window, cx) {
            s.push(layers);
        }
        if d.role != DspRole::Tuning {
            let this = this.clone();
            s.push(
                Button::new("profile-more", ButtonKind::Link)
                    .label(if self.more {
                        "Less"
                    } else {
                        "Bands, sync and more"
                    })
                    .on_click(move |_, _, cx| {
                        this.update(cx, |p, cx| {
                            p.more = !p.more;
                            cx.notify();
                        })
                        .ok();
                    })
                    .into_any_element(),
            );
        }
        if d.role == DspRole::Tuning || self.more {
            s.extend(self.more_sections(&d, window, cx));
        }
        {
            let (sheets, dsp, name) = (self.sheets.clone(), self.dsp.clone(), d.name.clone());
            let lines = self
                .dsp
                .read(cx)
                .profile(&d.name)
                .map(|p| {
                    let mut l = Vec::new();
                    if !p.used_on.is_empty() {
                        l.push(format!(
                            "Used on {}, which will play without it.",
                            joined(&p.used_on)
                        ));
                    }
                    if !p.held_by.is_empty() {
                        l.push(format!("It is taken out of {}.", joined(&p.held_by)));
                    }
                    l
                })
                .unwrap_or_default();
            s.push(
                Button::new("profile-delete", ButtonKind::Bordered)
                    .label("Delete")
                    .destructive(true)
                    .on_click(move |_, _, cx| {
                        let (dsp, n, sheets2) = (dsp.clone(), name.clone(), sheets.clone());
                        let mut message =
                            vec!["Its impulse responses are deleted with it.".to_owned()];
                        message.extend(lines.iter().cloned());
                        sheets.update(cx, |s, cx| {
                            s.confirm(
                                Confirm {
                                    title: format!("Delete {name}?").into(),
                                    message: message.join(" ").into(),
                                    action: "Delete".into(),
                                    destructive: true,
                                    also: None,
                                    run: Rc::new(move |_, cx| {
                                        dsp.update(cx, |d, cx| d.delete(n.clone(), cx));
                                        sheets2.update(cx, |s, cx| s.pop(cx));
                                    }),
                                },
                                cx,
                            )
                        });
                    })
                    .into_any_element(),
            );
        }
        body("profile-body", &look)
            .size_full()
            .pt(look.space(Space::M))
            .children(s)
            .into_any_element()
    }
}

impl ProfileBody {
    fn keeping(
        &self,
        d: &DspProfileDetail,
        look: &Look,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let this = cx.weak_entity();
        let mut rows: Vec<AnyElement> = Vec::new();
        match &self.copy_input {
            Some(input) => {
                let (go, cancel) = (this.clone(), this.clone());
                rows.push(
                    div()
                        .flex()
                        .gap(look.space(Space::M))
                        .child(div().flex_1().child(TextField::new(input)))
                        .child(
                            Button::new("copy-go", ButtonKind::Bordered)
                                .label("Save")
                                .on_click(move |_, window, cx| {
                                    go.update(cx, |p, cx| p.save_copy(window, cx)).ok();
                                }),
                        )
                        .child(
                            Button::new("copy-cancel", ButtonKind::Link)
                                .label("Cancel")
                                .on_click(move |_, _, cx| {
                                    cancel
                                        .update(cx, |p, cx| {
                                            p.copy_input = None;
                                            cx.notify();
                                        })
                                        .ok();
                                }),
                        )
                        .into_any_element(),
                );
                rows.push(note("The copy takes this EQ's place in the filters, and this one goes back to how it was.", Tone::Muted, look).into_any_element());
            }
            None => {
                let open = this.clone();
                let name = self.name.clone();
                let mut buttons = vec![
                    Button::new("copy-open", ButtonKind::Bordered)
                        .label("Save as Copy…")
                        .on_click(move |_, window, cx| {
                            let name = name.clone();
                            open.update(cx, |p, cx| {
                                p.copy_input = Some(cx.new(|cx| {
                                    InputState::new(window, cx)
                                        .default_value(format!("{name} copy"))
                                }));
                                cx.notify();
                            })
                            .ok();
                        })
                        .into_any_element(),
                ];
                if d.can_revert && d.edited {
                    let (dsp, name, this, source) = (
                        self.dsp.clone(),
                        self.name.clone(),
                        this.clone(),
                        d.source
                            .first()
                            .cloned()
                            .unwrap_or_else(|| "its file".into()),
                    );
                    let sheets = self.sheets.clone();
                    buttons.push(
                        Button::new("profile-reset", ButtonKind::Bordered)
                            .label("Reset to File")
                            .on_click(move |_, _, cx| {
                                let (dsp, n, this) = (dsp.clone(), name.clone(), this.clone());
                                sheets.update(cx, |s, cx| {
                                    s.confirm(
                                        Confirm {
                                            title: format!("Reset {name} to its file?").into(),
                                            message: format!("Puts it back as {source} had it, every filter and the headroom with it; the edits made since are lost.").into(),
                                            action: "Reset".into(),
                                            destructive: true,
                                            also: None,
                                            run: Rc::new(move |_, cx| {
                                                dsp.update(cx, |d, cx| d.revert(n.clone(), cx));
                                                this.update(cx, |p, _| p.before = None).ok();
                                            }),
                                        },
                                        cx,
                                    )
                                });
                            })
                            .into_any_element(),
                    );
                }
                rows.push(
                    div()
                        .flex()
                        .gap(look.space(Space::M))
                        .children(buttons)
                        .into_any_element(),
                );
            }
        }
        let footer = if d.can_revert {
            format!(
                "Save as Copy keeps this edit as an EQ of its own, in this one's place in the filters, and puts this one back. Reset to File puts it back as {} had it.",
                d.source
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "its file".into())
            )
        } else {
            "Save as Copy keeps this edit as an EQ of its own, in this one's place in the filters, and puts this one back as it was when you opened it.".to_owned()
        };
        let _ = window;
        section(None, rows, Some(footer.into()), look).into_any_element()
    }

    fn channels(
        &self,
        d: &DspProfileDetail,
        layout: koan_runtime::DspChannelLayout,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let this = cx.weak_entity();
        let (dsp, name, sheets) = (self.dsp.clone(), d.name.clone(), self.sheets.clone());
        let mut rows = vec![
            FormRow::new("Channels")
                .control(Segmented::new("profile-stereo", ["Mono", "Stereo"], layout.stereo as usize).on_change({
                    let this = this.clone();
                    move |i, _, cx| {
                        let stereo = *i == 1;
                        if stereo == layout.stereo {
                            return;
                        }
                        let (dsp, name) = (dsp.clone(), name.clone());
                        if !stereo && layout.differ {
                            let keep = |k: u16| {
                                let (dsp, name, this) = (dsp.clone(), name.clone(), this.clone());
                                Rc::new(move |_: &mut Window, cx: &mut App| {
                                    let name = name.clone();
                                    dsp.update(cx, |d, cx| d.act(cx, move |e| e.dsp_set_stereo(name, false, k)));
                                    this.update(cx, |p, _| p.channel = 0).ok();
                                })
                            };
                            let message = "The two channels have different bands. One list plays on both, and the other channel's bands are dropped.";
                            sheets.update(cx, |s, cx| {
                                s.confirm(
                                    Confirm {
                                        title: "Keep which channel?".into(),
                                        message: message.into(),
                                        action: "Keep Left".into(),
                                        destructive: true,
                                        also: Some(("Keep Right".into(), keep(1))),
                                        run: keep(0),
                                    },
                                    cx,
                                )
                            });
                            return;
                        }
                        dsp.update(cx, |d, cx| d.act(cx, move |e| e.dsp_set_stereo(name, stereo, 0)));
                        this.update(cx, |p, _| p.channel = 0).ok();
                    }
                }))
                .into_any_element(),
        ];
        if layout.stereo {
            rows.push(
                FormRow::new("Editing")
                    .control(
                        Segmented::new("profile-channel", ["Left", "Right"], self.channel as usize)
                            .on_change(move |i, _, cx| {
                                this.update(cx, |p, cx| {
                                    p.channel = *i as u16;
                                    p.loaded = None;
                                    p.load(cx);
                                })
                                .ok();
                            }),
                    )
                    .into_any_element(),
            );
        }
        let footer = if layout.stereo {
            "Each channel has bands of its own. A mono recording plays the left channel's; an output with more than two channels has them on its front left and right alone."
        } else {
            "One list of bands for both channels. Stereo starts each channel with a copy of it."
        };
        section(None, rows, Some(footer.into()), look).into_any_element()
    }

    /// What a profile is for, and for a correction, the target it corrects
    /// to.
    fn role_section(
        &self,
        d: &DspProfileDetail,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let look = Look::of(window, cx);
        let mut rows: Vec<AnyElement> = Vec::new();
        let roles = [DspRole::Correction, DspRole::Baked, DspRole::Tuning];
        let items = roles
            .iter()
            .map(|&r| {
                let (dsp, name, current) = (self.dsp.clone(), d.name.clone(), d.role);
                Item::choice(role_label(r), r == current, move |_, cx| {
                    if r != current {
                        let name = name.clone();
                        dsp.update(cx, |d, cx| d.act(cx, move |e| e.dsp_set_role(name, r)));
                    }
                })
            })
            .collect();
        rows.push(
            FormRow::new("This is")
                .control(menu(
                    "profile-role",
                    role_label(d.role),
                    Trigger::Field,
                    items,
                    window,
                    cx,
                ))
                .into_any_element(),
        );
        let (over, in_ear, added) = target_groups(&self.over, &self.in_ear);
        let first = self.dsp.read(cx).overview.as_ref().and_then(|o| o.in_ear);
        let groups = || {
            let mut g: Vec<(&str, &Vec<DspTargetOption>)> = match first {
                Some(true) => vec![("In-ear", &in_ear), ("Other", &over), ("Added", &added)],
                Some(false) => vec![("Over-ear", &over), ("Other", &in_ear), ("Added", &added)],
                None => vec![("Over-ear", &over), ("In-ear", &in_ear), ("Added", &added)],
            };
            g.retain(|(_, v)| !v.is_empty());
            g
        };
        let target_menu = |id: &'static str,
                           title: &str,
                           saved: Option<String>,
                           save: SaveTarget,
                           window: &mut Window,
                           cx: &mut App| {
            let mut items = vec![{
                let save = save.clone();
                Item::choice(label("Unknown"), saved.is_none(), move |_, cx| {
                    save(None, cx)
                })
            }];
            for (heading, ts) in groups() {
                items.push(Item::Heading(label(heading)));
                for t in ts {
                    let (save, id) = (save.clone(), t.id.clone());
                    items.push(Item::choice(
                        target_label(t, false),
                        saved.as_ref() == Some(&t.id),
                        move |_, cx| save(Some(id.clone()), cx),
                    ));
                }
            }
            let shown = saved
                .as_ref()
                .and_then(|s| {
                    over.iter()
                        .chain(&in_ear)
                        .chain(&added)
                        .find(|t| &t.id == s)
                })
                .map(|t| t.name.clone())
                .unwrap_or_else(|| label("Unknown").to_string());
            FormRow::new(title)
                .control(menu(id, shown, Trigger::Field, items, window, cx))
                .into_any_element()
        };
        let has_choices = !(over.is_empty() && in_ear.is_empty() && added.is_empty());
        if d.role == DspRole::Tuning && has_choices {
            let (dsp, name) = (self.dsp.clone(), d.name.clone());
            let save: SaveTarget = Rc::new(move |t, cx| {
                let name = name.clone();
                dsp.update(cx, |d, cx| d.act(cx, move |e| e.dsp_set_tuned_for(name, t)));
            });
            rows.push(target_menu(
                "profile-made-against",
                "Made against",
                d.tuned_for.clone(),
                save,
                window,
                cx,
            ));
            // On a phone each choice shows what it adds; here, the one
            // chosen is drawn on the graph, and the previews side by side.
            if !self.previews.is_empty() {
                let range = self
                    .previews
                    .values()
                    .flatten()
                    .fold(6.0f64, |m, v| m.max(v.abs()));
                // In the menu's order, Unknown first.
                let unknown = String::new();
                let order: Vec<&String> = std::iter::once(&unknown)
                    .chain(over.iter().chain(&in_ear).chain(&added).map(|t| &t.id))
                    .collect();
                let mut previews: Vec<(&String, &Vec<f64>)> = self.previews.iter().collect();
                previews.sort_by_key(|(id, _)| {
                    order.iter().position(|o| o == id).unwrap_or(usize::MAX)
                });
                let thumbs = previews
                    .into_iter()
                    .filter(|(_, db)| db.len() > 1)
                    .map(|(id, db)| {
                        let name = if id.is_empty() {
                            label("Unknown").to_string()
                        } else {
                            over.iter()
                                .chain(&in_ear)
                                .chain(&added)
                                .find(|t| &t.id == id)
                                .map(|t| t.name.clone())
                                .unwrap_or_else(|| id.clone())
                        };
                        div()
                            .flex()
                            .flex_col()
                            .gap(look.spaced(2.0))
                            .child(thumb(db.clone(), Stroke::eq(1), Some(range)))
                            .child(note(name, Tone::Muted, &look))
                    });
                rows.push(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(look.space(Space::M))
                        .children(thumbs.collect::<Vec<_>>())
                        .into_any_element(),
                );
            }
            if d.tuned_for.is_none()
                && let Some(s) = self.suggestion.clone()
            {
                let (dsp, name) = (self.dsp.clone(), d.name.clone());
                rows.push(
                    Button::new("profile-suggest", ButtonKind::Link)
                        .text(format!(
                            "{} {}. {}",
                            label("Looks made for"),
                            s.name,
                            label("Use that?")
                        ))
                        .on_click(move |_, _, cx| {
                            let (name, id) = (name.clone(), s.id.clone());
                            dsp.update(cx, |d, cx| {
                                d.act(cx, move |e| e.dsp_set_tuned_for(name, Some(id)))
                            });
                        })
                        .into_any_element(),
                );
            }
        }
        if d.role == DspRole::Baked && d.impulses.is_empty() && d.layers.is_empty() {
            let (dsp, sheets, name) = (self.dsp.clone(), self.sheets.clone(), d.name.clone());
            rows.push(
                Button::new("profile-split", ButtonKind::Bordered)
                    .label("Split into Correction + Tuning…")
                    .on_click(move |_, window, cx| {
                        let view = cx.new(|cx| {
                            super::flows::SplitFlow::new(
                                dsp.clone(),
                                sheets.clone(),
                                name.clone(),
                                window,
                                cx,
                            )
                        });
                        sheets.update(cx, |s, cx| s.view(view, cx));
                    })
                    .into_any_element(),
            );
        }
        if d.role == DspRole::Correction {
            if let Some(t) = self.targets.clone() {
                let made_for = t.made_for.as_ref().map(|m| m.id.clone());
                let current = t.chosen.clone().or(made_for.clone()).unwrap_or_default();
                let items = t
                    .choices
                    .iter()
                    .map(|c| {
                        let (dsp, name, id, made_for) = (
                            self.dsp.clone(),
                            d.name.clone(),
                            c.id.clone(),
                            made_for.clone(),
                        );
                        Item::choice(target_label(c, false), c.id == current, move |_, cx| {
                            let (name, id) = (
                                name.clone(),
                                (Some(&id) != made_for.as_ref()).then(|| id.clone()),
                            );
                            dsp.update(cx, |d, cx| {
                                d.act(cx, move |e| e.dsp_choose_target(name, id))
                            });
                        })
                    })
                    .collect();
                let chosen = t.choices.iter().find(|c| c.id == current);
                rows.push(
                    FormRow::new("Corrected to")
                        .control(menu(
                            "profile-target",
                            chosen.map(|c| c.name.clone()).unwrap_or_default(),
                            Trigger::Field,
                            items,
                            window,
                            cx,
                        ))
                        .into_any_element(),
                );
                if let Some(c) = chosen.filter(|c| !c.character.is_empty()) {
                    rows.push(note(c.character.clone(), Tone::Muted, &look).into_any_element());
                }
                let this = cx.weak_entity();
                rows.push(
                    Button::new("profile-add-target", ButtonKind::Link)
                        .label("Add a Target…")
                        .on_click(move |_, _, cx| {
                            this.update(cx, |p, cx| p.add_target(cx)).ok();
                        })
                        .into_any_element(),
                );
            }
            if self.targets.is_none() && !d.measured && has_choices {
                let (dsp, name) = (self.dsp.clone(), d.name.clone());
                let save: SaveTarget = Rc::new(move |t, cx| {
                    let name = name.clone();
                    dsp.update(cx, |d, cx| d.act(cx, move |e| e.dsp_set_made_for(name, t)));
                });
                rows.push(target_menu(
                    "profile-made-for",
                    "Made for",
                    d.made_for.clone(),
                    save,
                    window,
                    cx,
                ));
            }
        }
        section(
            Some("What it's for"),
            rows,
            Some(role_footer(d, self.targets.as_ref()).into()),
            &look,
        )
        .into_any_element()
    }

    /// A group's members, one playing, chosen as a radio button is.
    fn group_section(
        &self,
        d: &DspProfileDetail,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let look = Look::of(window, cx);
        let playing = d
            .layers
            .iter()
            .find(|l| l.on)
            .or(d.layers.first())
            .map(|l| l.profile.clone());
        let mut rows: Vec<AnyElement> = d
            .layers
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let (dsp, group, member) = (self.dsp.clone(), d.name.clone(), l.profile.clone());
                super::sheet::choice_row(
                    ("member", i),
                    l.profile.clone(),
                    None,
                    (playing.as_ref() == Some(&l.profile)).then(|| {
                        KoanIcon::new(Icon::Check)
                            .role(TypeRole::Body)
                            .tone(Tone::Accent)
                            .into_any_element()
                    }),
                    &look,
                    move |_, _, cx| {
                        let (group, member) = (group.clone(), member.clone());
                        dsp.update(cx, |d, cx| d.act(cx, move |e| e.dsp_select(group, member)));
                    },
                )
                .into_any_element()
            })
            .collect();
        let (dsp, name) = (self.dsp.clone(), d.name.clone());
        rows.push(
            Button::new("group-all", ButtonKind::Link)
                .label("Play Them All in Order")
                .on_click(move |_, _, cx| {
                    let name = name.clone();
                    dsp.update(cx, |d, cx| d.act(cx, move |e| e.dsp_set_group(name, false)));
                })
                .into_any_element(),
        );
        let footer = if d.layers.iter().any(|l| l.on) {
            "One member plays at a time. Pick another and it plays in place of the last. Each member is an EQ of its own, with its own page."
        } else {
            "None was picked, so the first plays. Pick one to change it."
        };
        section(Some("Group: pick one"), rows, Some(footer.into()), &look).into_any_element()
    }

    /// The profiles a stack plays first, in order, each switched on or off.
    fn layers_section(
        &self,
        d: &DspProfileDetail,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let look = Look::of(window, cx);
        let profiles = self
            .dsp
            .read(cx)
            .overview
            .as_ref()
            .map(|o| o.profiles.clone())
            .unwrap_or_default();
        let addable: Vec<_> = profiles
            .into_iter()
            .filter(|p| {
                p.name != d.name
                    && p.rates.is_empty()
                    && !d.layers.iter().any(|l| l.profile == p.name)
            })
            .collect();
        let layers = d.layers.clone();
        let set = {
            let (dsp, name) = (self.dsp.clone(), d.name.clone());
            Rc::new(move |layers: Vec<DspLayerInfo>, cx: &mut App| {
                let name = name.clone();
                dsp.update(cx, |d, cx| {
                    d.act(cx, move |e| e.dsp_set_layers(name, layers))
                });
            })
        };
        let add_menu = |window: &mut Window, cx: &mut App| {
            let mut items: Vec<Item> = addable
                .iter()
                .filter(|p| p.role == DspRole::Tuning)
                .map(|p| {
                    let (set, layers, name) = (set.clone(), layers.clone(), p.name.clone());
                    Item::action(p.name.clone(), move |_, cx| {
                        let mut l = layers.clone();
                        l.push(DspLayerInfo {
                            profile: name.clone(),
                            on: true,
                        });
                        set(l, cx);
                    })
                })
                .collect();
            let others: Vec<_> = addable
                .iter()
                .filter(|p| p.role != DspRole::Tuning)
                .collect();
            if !others.is_empty() {
                items.push(Item::Heading(label("Corrections")));
                for p in others {
                    let (set, layers, name) = (set.clone(), layers.clone(), p.name.clone());
                    items.push(Item::action(
                        format!("{} · {}", p.name, role_label(p.role)),
                        move |_, cx| {
                            let mut l = layers.clone();
                            l.push(DspLayerInfo {
                                profile: name.clone(),
                                on: true,
                            });
                            set(l, cx);
                        },
                    ));
                }
            }
            menu(
                "layers-add",
                label(if layers.is_empty() {
                    "Add Filters…"
                } else {
                    "Add an EQ"
                }),
                Trigger::Field,
                items,
                window,
                cx,
            )
        };
        if layers.is_empty() {
            return (!addable.is_empty()).then(|| add_menu(window, cx));
        }
        let mut rows: Vec<AnyElement> = Vec::new();
        for (i, layer) in layers.iter().enumerate() {
            let toggle = {
                let (set, layers) = (set.clone(), layers.clone());
                Toggle::new(("layer-on", i), "", layer.on).on_change(move |on, _, cx| {
                    let mut l = layers.clone();
                    l[i].on = *on;
                    set(l, cx);
                })
            };
            let mut items = Vec::new();
            if i > 0 {
                let (set, layers) = (set.clone(), layers.clone());
                items.push(Item::action(label("Move Up"), move |_, cx| {
                    let mut l = layers.clone();
                    l.swap(i, i - 1);
                    set(l, cx);
                }));
            }
            if i + 1 < layers.len() {
                let (set, layers) = (set.clone(), layers.clone());
                items.push(Item::action(label("Move Down"), move |_, cx| {
                    let mut l = layers.clone();
                    l.swap(i, i + 1);
                    set(l, cx);
                }));
            }
            {
                let (set, layers) = (set.clone(), layers.clone());
                items.push(Item::action(label("Remove"), move |_, cx| {
                    let mut l = layers.clone();
                    l.remove(i);
                    set(l, cx);
                }));
            }
            let role = d.layer_roles.get(i).copied().flatten();
            rows.push(
                div()
                    .flex()
                    .items_center()
                    .gap(look.space(Space::M))
                    .child(toggle)
                    .child(
                        div()
                            .flex_1()
                            .text_role(TypeRole::Body, &look)
                            .text_color(look.palette.ink.hsla())
                            .child(layer.profile.clone()),
                    )
                    .children(
                        role.map(|r| Badge::new(role_label(r)).accent(r == DspRole::Correction)),
                    )
                    .child(menu(
                        ("layer-options", i),
                        label("Options"),
                        Trigger::Word,
                        items,
                        window,
                        cx,
                    ))
                    .into_any_element(),
            );
        }
        if !addable.is_empty() {
            rows.push(add_menu(window, cx));
        }
        if layers.len() > 1 {
            let (dsp, name) = (self.dsp.clone(), d.name.clone());
            rows.push(
                Button::new("layers-group", ButtonKind::Link)
                    .label("Make It a Group, One Playing at a Time")
                    .on_click(move |_, _, cx| {
                        let name = name.clone();
                        dsp.update(cx, |d, cx| d.act(cx, move |e| e.dsp_set_group(name, true)));
                    })
                    .into_any_element(),
            );
        }
        Some(
            section(
                Some("Plays first"),
                rows,
                Some("Played in order, before this EQ's own bands. One switched off plays nothing. Removing one takes it out of this list and keeps the EQ.".into()),
                &look,
            )
            .into_any_element(),
        )
    }

    fn more_sections(
        &mut self,
        d: &DspProfileDetail,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let look = Look::of(window, cx);
        let mut out: Vec<AnyElement> = Vec::new();
        if self.dsp.read(cx).offers(SYNC_EXTENSION) {
            let mut rows: Vec<AnyElement> = Vec::new();
            let (dsp, name, sheets, everywhere) = (
                self.dsp.clone(),
                d.name.clone(),
                self.sheets.clone(),
                d.everywhere,
            );
            rows.push(
                FormRow::new("Sync")
                    .control(Segmented::new("profile-scope", ["Everywhere", "This device"], (!everywhere) as usize).on_change(move |i, _, cx| {
                        let to = *i == 0;
                        if to == everywhere {
                            return;
                        }
                        let (dsp, n) = (dsp.clone(), name.clone());
                        let message = if to {
                            format!("{name} is sent to every device signed in to your kōan server, and an edit on one reaches the rest.")
                        } else {
                            format!("{name} stays on this device and is removed from your other devices.")
                        };
                        sheets.update(cx, |s, cx| {
                            s.confirm(
                                Confirm {
                                    title: label(if to { "Keep on Every Device" } else { "Keep on This Device Only" }),
                                    message: message.into(),
                                    action: if to { "Keep Everywhere".into() } else { "Keep Here".into() },
                                    destructive: !to,
                                    also: None,
                                    run: Rc::new(move |_, cx| {
                                        let n = n.clone();
                                        dsp.update(cx, |d, cx| d.act(cx, move |e| e.dsp_set_scope(n, to)));
                                    }),
                                },
                                cx,
                            )
                        });
                    }))
                    .into_any_element(),
            );
            if let Some(p) = &d.sync_problem {
                rows.push(warning(p.clone(), &look));
            }
            if let Some(n) = &d.sync_note {
                rows.push(note(n.clone(), Tone::Muted, &look).into_any_element());
            }
            let footer = if d.everywhere {
                "Everywhere: kept on every device signed in to your kōan server, and an edit on one reaches the rest. Which output plays it stays each device's own. Headphone corrections sync by default, since headphones move between devices."
            } else {
                "This device: never leaves it. Room and speaker corrections stay by default, since they belong to where they were measured. Moving one here from everywhere removes it from your other devices."
            };
            out.push(section(Some("Sync"), rows, Some(footer.into()), &look).into_any_element());
        }
        if !d.impulses.is_empty() {
            let rows = d
                .impulses
                .iter()
                .map(|ir| {
                    let channels = match ir.channels {
                        None => "Every channel".to_owned(),
                        Some(1) => "Mono".to_owned(),
                        Some(2) => "Stereo".to_owned(),
                        Some(n) => format!("{n} channels"),
                    };
                    let seconds = ir.taps as f64 / ir.rate as f64;
                    let mut shape = vec![
                        format!("{} taps ({seconds:.2} s)", ir.taps),
                        format!("peaks at {:.1} ms", ir.peak_ms),
                    ];
                    if ir.mixes {
                        shape.push("mixes channels".into());
                    }
                    if ir.delayed {
                        shape.push("delays channels".into());
                    }
                    div()
                        .flex()
                        .flex_col()
                        .py(look.spaced(2.0))
                        .child(
                            div()
                                .flex()
                                .justify_between()
                                .text_role(TypeRole::Meta, &look)
                                .text_color(look.palette.ink.hsla())
                                .child(format!("{} kHz", khz(ir.rate)))
                                .child(
                                    div()
                                        .text_color(look.text_color(Tone::Muted))
                                        .child(channels),
                                ),
                        )
                        .child(note(shape.join(" · "), Tone::Muted, &look))
                        .child(note(ir.file.clone(), Tone::Muted, &look))
                        .into_any_element()
                })
                .collect();
            out.push(
                section(Some("Impulse responses"), rows, Some("A track at a rate with no response of its own is resampled to the nearest one here.".into()), &look)
                    .into_any_element(),
            );
        }
        {
            let this = cx.weak_entity();
            let open: bands::RowAction = Rc::new(move |i, _, cx| {
                this.update(cx, |p, cx| {
                    p.curve = Some(i);
                    cx.notify();
                })
                .ok();
            });
            let (dsp, sheets, name) = (self.dsp.clone(), self.sheets.clone(), d.name.clone());
            let remove: bands::RemoveAction = Rc::new(move |i, position, band, _, cx| {
                let (dsp, n) = (dsp.clone(), name.clone());
                let kinds = dsp.read(cx).bounds.kinds.clone();
                let what = format!(
                    "{} {}",
                    bands::kind_name(&band, &kinds),
                    bands::values(&band)
                );
                sheets.update(cx, |s, cx| {
                    s.confirm(
                        Confirm {
                            title: format!("Remove filter {position}?", position = position + 1).into(),
                            message: format!("{what} is taken out of {n}, which plays without it.").into(),
                            action: "Remove".into(),
                            destructive: true,
                            also: None,
                            run: Rc::new(move |_, cx| {
                                let (n, band) = (n.clone(), band.clone());
                                dsp.update(cx, |d, cx| {
                                    d.act(cx, move |e| async move {
                                        match e.dsp_remove_edited(n, i as u32, band).await? {
                                            koan_runtime::DspEdited::Changed => Err(koan_runtime::KoanError::Refused {
                                                message: "The filters changed since they were shown, so nothing was removed. They are shown again as they are now.".into(),
                                            }),
                                            koan_runtime::DspEdited::Stored { .. } => Ok(()),
                                        }
                                    })
                                });
                            }),
                        },
                        cx,
                    )
                });
            });
            let channel = self.stereo_channel();
            let read_at = self.read_at();
            let table = bands::table(
                &mut self.fields,
                &d.bands,
                d.read_only,
                channel,
                read_at,
                open,
                remove,
                window,
                cx,
            );
            let mut rows = vec![table];
            if !d.read_only {
                let (dsp, name) = (self.dsp.clone(), d.name.clone());
                rows.push(
                    Button::new("band-add", ButtonKind::Bordered)
                        .label("Add a Band")
                        .on_click(move |_, _, cx| {
                            let name = name.clone();
                            dsp.update(cx, |d, cx| {
                                d.act(cx, move |e| async move {
                                    e.dsp_add_band(name, channel).await.map(|_| ())
                                })
                            });
                        })
                        .into_any_element(),
                );
            }
            out.push(
                section(
                    Some("Filters"),
                    rows,
                    Some(bands::footer(d.read_only)),
                    &look,
                )
                .into_any_element(),
            );
        }
        out.push(
            section(
                Some("Headroom"),
                vec![FormRow::new("Preamp").value(format!("{:.1} dB", d.preamp_db)).into_any_element()],
                Some(if d.preamp_set {
                    "Set in this EQ.".into()
                } else {
                    format!("Derived at {} kHz from the largest gain the filters apply, so nothing they boost can clip.", khz(d.preamp_rate)).into()
                }),
                &look,
            )
            .into_any_element(),
        );
        if !d.source.is_empty() {
            let rows = d
                .source
                .iter()
                .map(|s| note(s.clone(), Tone::Muted, &look).into_any_element())
                .collect();
            out.push(section(Some("Imported from"), rows, None, &look).into_any_element());
        }
        out
    }

    fn curve_page(
        &mut self,
        index: usize,
        d: &DspProfileDetail,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let look = Look::of(window, cx);
        let band = d.bands[index].clone();
        let read_at = self.read_at();
        let points = bands::points(&mut self.fields, index, &band, read_at, window, cx);
        let mut s = vec![SectionHeader::new("Points").into_any_element(), points];
        if d.can_revert && d.edited {
            let (dsp, sheets, name, source) = (
                self.dsp.clone(),
                self.sheets.clone(),
                d.name.clone(),
                d.source
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "its file".into()),
            );
            s.push(
                Button::new("curve-reset", ButtonKind::Bordered)
                    .label("Reset to File")
                    .on_click(move |_, _, cx| {
                        let (dsp, n) = (dsp.clone(), name.clone());
                        sheets.update(cx, |s, cx| {
                            s.confirm(
                                Confirm {
                                    title: format!("Reset {name} to its file?").into(),
                                    message: format!("Puts this EQ back as {source} had it, every filter and the headroom with it.").into(),
                                    action: "Reset".into(),
                                    destructive: true,
                                    also: None,
                                    run: Rc::new(move |_, cx| dsp.update(cx, |d, cx| d.revert(n.clone(), cx))),
                                },
                                cx,
                            )
                        });
                    })
                    .into_any_element(),
            );
        }
        body("curve-body", &look)
            .size_full()
            .pt(look.space(Space::M))
            .children(s)
            .into_any_element()
    }
}

/// Saves a target chosen from a menu, or Unknown with none.
type SaveTarget = Rc<dyn Fn(Option<String>, &mut App)>;

fn role_label(r: DspRole) -> String {
    label(match r {
        DspRole::Correction => "Correction",
        DspRole::Tuning => "Tuning",
        DspRole::Baked => "Correction + Tuning",
    })
    .to_string()
}

fn role_footer(d: &DspProfileDetail, targets: Option<&DspTargets>) -> String {
    match d.role {
        DspRole::Tuning => "A tuning is taste: more bass, a darker treble. It plays on top of a correction. Say which target it was made against, and on a device corrected to another, kōan plays the difference first, so it sounds as it was made to.".into(),
        DspRole::Baked => "A correction with a tuning already in it, as most finished presets are. It is the device's correction, so another tuning on top would add taste twice. Split it, with a measurement of the device, to change the tuning.".into(),
        DspRole::Correction if d.measured => "A correction makes your headphones or speakers neutral, and the target says what neutral is. This one is worked out again from the measurement for each target.".into(),
        DspRole::Correction => match targets.and_then(|t| t.made_for.as_ref()) {
            Some(made) => format!("Made for {}. Another target is worked out from the measurement AutoEQ kept, where there is one, or plays as the difference between the two. Moving from Harman to neutral takes Harman's bass and treble out.", made.name),
            None => "A correction makes your headphones or speakers neutral. Say which target this EQ was made for, and you can move it to another; if you don't know, leave it Unknown and target switching stays off.".into(),
        },
    }
}

/// Every target by the headphones it is for, and those listed for both,
/// which are the ones added: a menu with a row twice cannot choose either.
pub fn target_groups(
    over: &[DspTargetOption],
    in_ear: &[DspTargetOption],
) -> (
    Vec<DspTargetOption>,
    Vec<DspTargetOption>,
    Vec<DspTargetOption>,
) {
    let both: std::collections::HashSet<&String> = over
        .iter()
        .map(|t| &t.id)
        .filter(|id| in_ear.iter().any(|t| &t.id == *id))
        .collect();
    (
        over.iter()
            .filter(|t| !both.contains(&t.id))
            .cloned()
            .collect(),
        in_ear
            .iter()
            .filter(|t| !both.contains(&t.id))
            .cloned()
            .collect(),
        over.iter()
            .filter(|t| both.contains(&t.id))
            .cloned()
            .collect(),
    )
}

/// What a profile is for, in a line each: the headphone the chain corrects
/// and how, and the tuning on top.
fn summary(d: &DspProfileDetail, look: &Look) -> AnyElement {
    let line = |role: DspRole, text: String, muted: bool| {
        div()
            .flex()
            .items_baseline()
            .gap(look.spaced(6.0))
            .child(Badge::new(role_label(role)).accent(role == DspRole::Correction))
            .child(
                div()
                    .text_role(TypeRole::Body, look)
                    .text_color(if muted {
                        look.text_color(Tone::Muted)
                    } else {
                        look.palette.ink.hsla()
                    })
                    .child(text),
            )
    };
    div()
        .flex()
        .flex_col()
        .gap(look.spaced(4.0))
        .child(match &d.corrects {
            Some(c) => line(
                if d.corrects_baked {
                    DspRole::Baked
                } else {
                    DspRole::Correction
                },
                c.clone(),
                false,
            ),
            None => line(DspRole::Correction, "None".into(), true),
        })
        .when(!d.tunings.is_empty(), |el| {
            el.child(line(DspRole::Tuning, d.tunings.join(", "), false))
        })
        .into_any_element()
}

fn warning(text: impl Into<SharedString>, look: &Look) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(look.space(Space::S))
        .when(look.icons, |d| {
            d.child(
                KoanIcon::new(Icon::WarningFilled)
                    .role(TypeRole::Meta)
                    .tone(Tone::Bad),
            )
        })
        .child(note(text, Tone::Bad, look))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(id: &str) -> DspTargetOption {
        DspTargetOption {
            id: id.into(),
            name: id.into(),
            does: String::new(),
            character: String::new(),
        }
    }

    #[test]
    fn a_target_listed_for_both_ears_is_listed_once_as_added() {
        let (over, in_ear, added) =
            target_groups(&[t("harman-oe"), t("mine")], &[t("harman-ie"), t("mine")]);
        let ids = |v: &[DspTargetOption]| v.iter().map(|t| t.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&over), ["harman-oe"]);
        assert_eq!(ids(&in_ear), ["harman-ie"]);
        assert_eq!(ids(&added), ["mine"]);
    }
}
