//! Choosing what fills a stage of a device's chain, the words the EQ page
//! uses, and making a new EQ.

use koan_ffi::{
    DspJoin, DspOverview, DspProfileSummary, DspRole, DspStarter, DspTargets, DspTuningEntry,
};
use koan_kit::app::*;
// gpui-component's text input: koan-kit's TextField draws it, the page owns its state.
use koan_kit::raw::component::input::{InputEvent, InputState};

use super::flows;
use super::menu::{Item, Trigger, menu};
use super::model::{Dsp, Placement, Stage, describe_profile};
use super::page::EqPage;
use super::sheet::{Sheets, body, choice_row, head, note, section};
use crate::components::{Badge, Button, ButtonKind, FormRow, Segmented, TextField};
use crate::theme::{Icon, KoanIcon, KoanStyled, Look, Space, Tone, TypeRole, label};

/// What fills a stage, chosen: a correction, with the target it aims at and
/// a group's member, or an EQ to add to the filters. Add… at the foot brings
/// in another.
pub struct StagePicker {
    dsp: Entity<Dsp>,
    sheets: Entity<Sheets>,
    stage: Stage,
    device: String,
    overview: Option<DspOverview>,
    targets: Option<DspTargets>,
    loaded: Option<(u64, u64)>,
    /// The read under way; a newer one drops it.
    _load: Option<koan_kit::app::Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl StagePicker {
    pub fn new(
        dsp: Entity<Dsp>,
        sheets: Entity<Sheets>,
        stage: Stage,
        device: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscriptions = vec![cx.observe(&dsp, |p: &mut Self, _, cx| p.load(cx))];
        let mut picker = Self {
            dsp,
            sheets,
            stage,
            device,
            overview: None,
            targets: None,
            loaded: None,
            _load: None,
            _subscriptions: subscriptions,
        };
        picker.load(cx);
        picker
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let stamp = self.dsp.read(cx).stamp();
        if self.loaded == Some(stamp) {
            return;
        }
        self.loaded = Some(stamp);
        let engine = self.dsp.read(cx).engine();
        let device = self.device.clone();
        self._load = Some(cx.spawn(async move |this, cx| {
            let o = engine.clone().dsp_overview_for(Some(device)).await;
            let targets = match &o.active {
                Some(name) => engine.dsp_targets(name.clone()).await,
                None => None,
            };
            this.update(cx, |p, cx| {
                p.overview = Some(o);
                p.targets = targets;
                cx.notify();
            })
            .ok();
        }));
    }

    fn close(&self, cx: &mut App) {
        self.sheets.update(cx, |s, cx| s.pop(cx));
    }

    /// The choices: corrections for the correction, EQs not yet in the
    /// filters for an EQ.
    fn choices(&self, o: &DspOverview) -> Vec<DspProfileSummary> {
        match self.stage {
            Stage::Correction => o
                .profiles
                .iter()
                .filter(|p| !p.preset && p.role != DspRole::Tuning)
                .cloned()
                .collect(),
            Stage::Eq => o
                .profiles
                .iter()
                .filter(|p| {
                    !p.preset
                        && p.role == DspRole::Tuning
                        && p.rates.is_empty()
                        && !o.chain.iter().any(|e| e.name == p.name)
                })
                .cloned()
                .collect(),
        }
    }

    /// The EQs made against the correction's target first, each group
    /// alphabetical; one list where there is no correction to match.
    fn groups(&self, o: &DspOverview) -> Vec<(&'static str, Vec<DspProfileSummary>)> {
        let mut choices = self.choices(o);
        if choices.is_empty() {
            return Vec::new();
        }
        if o.aim.is_none() || o.filters_only {
            return vec![("EQs", choices)];
        }
        choices.sort_by_key(|p| p.name.to_lowercase());
        let (matching, other): (Vec<_>, Vec<_>) = choices
            .into_iter()
            .partition(|p| matches!(p.join, Some(DspJoin::Matched)));
        if matching.is_empty() {
            return vec![("EQs", other)];
        }
        let mut groups = vec![("Matches your correction", matching)];
        if !other.is_empty() {
            groups.push(("Other", other));
        }
        groups
    }

    fn add(&self, what: Add, window: &mut Window, cx: &mut App) {
        let (dsp, sheets, device, stage) = (
            self.dsp.clone(),
            self.sheets.clone(),
            self.device.clone(),
            self.stage,
        );
        self.close(cx);
        match what {
            Add::Import => EqPage::import(dsp, Some(Placement { device, stage }), cx),
            Add::AutoEq => {
                let view = cx.new(|cx| {
                    flows::AutoEqSearch::new(dsp, sheets.clone(), String::new(), window, cx)
                });
                sheets.update(cx, |s, cx| s.view(view, cx));
            }
            Add::Measure => {
                let view = cx.new(|cx| {
                    flows::MeasurementFlow::new(dsp, sheets.clone(), String::new(), window, cx)
                });
                sheets.update(cx, |s, cx| s.view(view, cx));
            }
            Add::Split(name) => {
                let view =
                    cx.new(|cx| flows::SplitFlow::new(dsp, sheets.clone(), name, window, cx));
                sheets.update(cx, |s, cx| s.view(view, cx));
            }
            Add::New => {
                let view = cx.new(|cx| NewEq::new(dsp, sheets.clone(), Some(device), window, cx));
                sheets.update(cx, |s, cx| s.view(view, cx));
            }
        }
    }
}

enum Add {
    Import,
    AutoEq,
    Measure,
    New,
    Split(String),
}

impl Render for StagePicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let this = cx.weak_entity();
        let done = Button::new("picker-done", ButtonKind::Standard)
            .label("Done")
            .on_click({
                let this = this.clone();
                move |_, _, cx| {
                    this.update(cx, |p, cx| p.close(cx)).ok();
                }
            });
        let title = match self.stage {
            Stage::Correction => "Correction",
            Stage::Eq => "Add EQ",
        };
        let Some(o) = self.overview.clone() else {
            return div()
                .child(head(title, vec![done.into_any_element()], &look))
                .into_any_element();
        };
        let mut sections: Vec<AnyElement> = Vec::new();
        let device = self.device.clone();
        match self.stage {
            Stage::Correction => {
                let mut rows = vec![{
                    let (dsp, device, this) = (self.dsp.clone(), device.clone(), this.clone());
                    choice_row(
                        "none",
                        label("None"),
                        Some(label("No correction: the device as it is")),
                        chosen(o.active.is_none()),
                        &look,
                        move |_, _, cx| {
                            dsp.update(cx, |d, cx| d.assign(None, device.clone(), cx));
                            this.update(cx, |p, cx| p.close(cx)).ok();
                        },
                    )
                    .into_any_element()
                }];
                for (i, p) in self.choices(&o).into_iter().enumerate() {
                    let (dsp, device, this, name) = (
                        self.dsp.clone(),
                        device.clone(),
                        this.clone(),
                        p.name.clone(),
                    );
                    rows.push(
                        choice_row(
                            ("correction", i),
                            p.name.clone(),
                            Some(describe_profile(&p).into()),
                            chosen(o.active.as_ref() == Some(&p.name)),
                            &look,
                            move |_, _, cx| {
                                dsp.update(cx, |d, cx| {
                                    d.assign(Some(name.clone()), device.clone(), cx)
                                });
                                this.update(cx, |p, cx| p.close(cx)).ok();
                            },
                        )
                        .into_any_element(),
                    );
                }
                sections.push(section(Some("Corrections"), rows, None, &look).into_any_element());
                if let Some(name) = o.active.clone() {
                    sections.push(self.current(&o, &name, window, cx));
                }
            }
            Stage::Eq => {
                let groups = self.groups(&o);
                if groups.is_empty() {
                    sections.push(
                        section(
                            Some("EQs"),
                            vec![note("Every EQ is in the filters already. Make a new one or add another below.", Tone::Muted, &look).into_any_element()],
                            None,
                            &look,
                        )
                        .into_any_element(),
                    );
                }
                for (g, (title, eqs)) in groups.into_iter().enumerate() {
                    let rows = eqs
                        .into_iter()
                        .enumerate()
                        .map(|(i, p)| {
                            let (dsp, device, this, chain, name) = (
                                self.dsp.clone(),
                                device.clone(),
                                this.clone(),
                                o.chain.clone(),
                                p.name.clone(),
                            );
                            let made = (!o.filters_only).then(|| p.made_for.clone()).flatten();
                            let matched = matches!(p.join, Some(DspJoin::Matched));
                            let detail = describe_profile(&p);
                            div()
                                .flex()
                                .flex_col()
                                .child(choice_row(
                                    ("eq-choice", g * 1000 + i),
                                    p.name.clone(),
                                    (!detail.is_empty()).then(|| detail.into()),
                                    None,
                                    &look,
                                    move |_, _, cx| {
                                        let mut tuning = chain.clone();
                                        tuning.push(DspTuningEntry {
                                            name: name.clone(),
                                            on: true,
                                        });
                                        dsp.update(cx, |d, cx| {
                                            d.set_tunings(tuning, device.clone(), cx)
                                        });
                                        this.update(cx, |p, cx| p.close(cx)).ok();
                                    },
                                ))
                                .children(
                                    made.map(|m| div().flex().child(Badge::new(m).accent(matched))),
                                )
                                .into_any_element()
                        })
                        .collect();
                    sections.push(section(Some(title), rows, None, &look).into_any_element());
                }
            }
        }
        let add = |id: &'static str, text: &str, what: fn() -> Add| {
            let this = this.clone();
            Button::new(id, ButtonKind::Bordered)
                .label(text)
                .on_click(move |_, window, cx| {
                    this.update(cx, |p, cx| p.add(what(), window, cx)).ok();
                })
        };
        let mut adds: Vec<AnyElement> = Vec::new();
        if self.stage == Stage::Eq {
            adds.push(add("add-new", "New EQ…", || Add::New).into_any_element());
        }
        adds.push(add("add-import", "Import a File…", || Add::Import).into_any_element());
        if !o.filters_only {
            adds.push(add("add-autoeq", "Find in AutoEQ…", || Add::AutoEq).into_any_element());
            adds.push(
                add("add-measure", "Find a Measurement…", || Add::Measure).into_any_element(),
            );
        }
        sections.push(
            section(
                Some("Add…"),
                vec![
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(look.space(Space::M))
                        .children(adds)
                        .into_any_element(),
                ],
                None,
                &look,
            )
            .into_any_element(),
        );
        div()
            .flex()
            .flex_col()
            .child(head(title, vec![done.into_any_element()], &look))
            .child(body("picker-body", &look).children(sections))
            .into_any_element()
    }
}

impl StagePicker {
    /// The correction chosen: the target it aims at, a group's member, or
    /// for one with a tuning baked in, the way to take it apart.
    fn current(
        &self,
        o: &DspOverview,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let look = Look::of(window, cx);
        let correction = o.profiles.iter().find(|p| p.name == name).cloned();
        let mut rows: Vec<AnyElement> = Vec::new();
        if let (Some(t), Some(c)) = (&self.targets, &correction)
            && c.role == DspRole::Correction
        {
            let current = t
                .chosen
                .clone()
                .or_else(|| t.made_for.as_ref().map(|m| m.id.clone()))
                .unwrap_or_default();
            let default = t.made_for.as_ref().map(|m| m.id.clone());
            let items = t
                .choices
                .iter()
                .map(|choice| {
                    let (dsp, name, id, default) = (
                        self.dsp.clone(),
                        name.to_owned(),
                        choice.id.clone(),
                        default.clone(),
                    );
                    let shown =
                        target_label(choice, default.as_deref() == Some(choice.id.as_str()));
                    Item::choice(shown, choice.id == current, move |_, cx| {
                        let id = (Some(&id) != default.as_ref()).then(|| id.clone());
                        let name = name.clone();
                        dsp.update(cx, |d, cx| {
                            d.act(cx, move |e| e.dsp_choose_target(name, id))
                        });
                    })
                })
                .collect();
            let shown = t
                .choices
                .iter()
                .find(|c| c.id == current)
                .map(|c| c.name.clone())
                .unwrap_or_default();
            rows.push(
                FormRow::new("Target")
                    .control(menu(
                        "picker-target",
                        shown,
                        Trigger::Field,
                        items,
                        window,
                        cx,
                    ))
                    .into_any_element(),
            );
        }
        if let Some(c) = &correction
            && !c.members.is_empty()
        {
            let playing = c
                .playing
                .clone()
                .or_else(|| c.members.first().cloned())
                .unwrap_or_default();
            let items = c
                .members
                .iter()
                .map(|m| {
                    let (dsp, group, member) = (self.dsp.clone(), name.to_owned(), m.clone());
                    Item::choice(m.clone(), *m == playing, move |_, cx| {
                        let (group, member) = (group.clone(), member.clone());
                        dsp.update(cx, |d, cx| d.act(cx, move |e| e.dsp_select(group, member)));
                    })
                })
                .collect();
            rows.push(
                FormRow::new("Playing")
                    .control(menu(
                        "picker-member",
                        playing,
                        Trigger::Field,
                        items,
                        window,
                        cx,
                    ))
                    .into_any_element(),
            );
        }
        if let Some(c) = &correction
            && c.role == DspRole::Baked
        {
            rows.push(
                note(
                    format!("{name} already includes a tuning, so no other tuning plays on it. Split it into a correction and a tuning to change that."),
                    Tone::Muted,
                    &look,
                )
                .into_any_element(),
            );
            if c.rates.is_empty() && c.layers == 0 {
                let (this, name) = (cx.weak_entity(), name.to_owned());
                rows.push(
                    Button::new("picker-split", ButtonKind::Compact)
                        .label("Split into Correction + Tuning…")
                        .on_click(move |_, window, cx| {
                            this.update(cx, |p, cx| p.add(Add::Split(name.clone()), window, cx))
                                .ok();
                        })
                        .into_any_element(),
                );
            }
        }
        div()
            .flex()
            .flex_col()
            .gap(look.space(Space::Xs))
            .child(crate::components::SectionHeader::new(name))
            .children(rows)
            .into_any_element()
    }
}

/// A target as a menu lists it: its name, its default marked, and what it
/// does.
pub fn target_label(t: &koan_ffi::DspTargetOption, is_default: bool) -> String {
    let name = if is_default {
        format!("{} (default)", t.name)
    } else {
        t.name.clone()
    };
    if t.does.is_empty() {
        name
    } else {
        format!("{name}: {}", t.does)
    }
}

fn chosen(chosen: bool) -> Option<AnyElement> {
    chosen.then(|| {
        KoanIcon::new(Icon::Check)
            .role(TypeRole::Body)
            .tone(Tone::Accent)
            .into_any_element()
    })
}

/// What the words on the EQ page mean.
pub struct Explainer {
    sheets: Entity<Sheets>,
}

impl Explainer {
    pub fn new(sheets: Entity<Sheets>, _window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self { sheets }
    }
}

const WORDS: [(&str, &str); 5] = [
    (
        "Correction",
        "Makes your headphones neutral: measured and brought to a target. A device has one, and it plays as it was made, so it is not edited here; filters go on top. Switch Correct for a headphone profile off to play filters alone, as speakers usually do.",
    ),
    (
        "Filters",
        "Your EQs on top of the correction, or on their own: one or more, played in order, each switched on or off.",
    ),
    (
        "EQ",
        "One set of bands. Open it from the chain to edit it, or make a new one from Add EQ.",
    ),
    (
        "Preset",
        "A device's correction and filters saved together, to switch it between, or set another device from.",
    ),
    (
        "Flat",
        "Nothing chosen: the music plays untouched, bit for bit.",
    ),
];

impl Render for Explainer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let sheets = self.sheets.clone();
        let done = Button::new("explain-done", ButtonKind::Standard)
            .label("Done")
            .on_click(move |_, _, cx| sheets.update(cx, |s, cx| s.pop(cx)));
        div()
            .flex()
            .flex_col()
            .child(head("How EQ works", vec![done.into_any_element()], &look))
            .child(
                body("explain-body", &look)
                    .child(
                        div()
                            .text_role(TypeRole::Body, &look)
                            .text_color(look.palette.ink.hsla())
                            .child("Music plays through the correction, then the filters' EQs in order, then out to the device. Each device has its own."),
                    )
                    .child(crate::components::SectionHeader::new("Words"))
                    .children(WORDS.iter().map(|(word, meaning)| {
                        div()
                            .flex()
                            .flex_col()
                            .gap(look.space(Space::Xs))
                            .child(div().text_role(TypeRole::Body, &look).text_color(look.palette.strong.hsla()).child(label(word)))
                            .child(note(*meaning, Tone::Muted, &look))
                    }))
                    .child(note(
                        "An EQ made against one target and played on a correction aiming at another gets the difference between the two added, so it sounds as it was made.",
                        Tone::Muted,
                        &look,
                    )),
            )
    }
}

/// A new EQ: eight bands across the range, flat or from a starting curve,
/// named before it is made. Made for a device, it goes on the end of that
/// device's filters and opens to edit.
pub struct NewEq {
    dsp: Entity<Dsp>,
    sheets: Entity<Sheets>,
    device: Option<String>,
    name: Entity<InputState>,
    starter: usize,
    refusal: Option<SharedString>,
    saving: bool,
    _subscriptions: Vec<Subscription>,
}

const STARTERS: [(&str, DspStarter); 4] = [
    ("Flat", DspStarter::Flat),
    ("Bass boost", DspStarter::BassBoost),
    ("Warm", DspStarter::Warm),
    ("Bright", DspStarter::Bright),
];

impl NewEq {
    pub fn new(
        dsp: Entity<Dsp>,
        sheets: Entity<Sheets>,
        device: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder(label("Name")));
        let subscriptions = vec![cx.subscribe_in(
            &name,
            window,
            |this: &mut Self, _, e: &InputEvent, window, cx| match e {
                InputEvent::PressEnter { .. } => this.create(window, cx),
                InputEvent::Change => cx.notify(),
                _ => {}
            },
        )];
        Self {
            dsp,
            sheets,
            device,
            name,
            starter: 0,
            refusal: None,
            saving: false,
            _subscriptions: subscriptions,
        }
    }

    fn trimmed(&self, cx: &App) -> String {
        self.name.read(cx).value().trim().to_owned()
    }

    fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.trimmed(cx);
        if name.is_empty() || self.saving {
            return;
        }
        self.saving = true;
        let starter = STARTERS[self.starter].1;
        let this = cx.weak_entity();
        let made = name.clone();
        let window = window.window_handle();
        self.dsp.update(cx, |dsp, cx| {
            dsp.act_then(
                cx,
                move |engine| engine.dsp_create_eq(made, starter),
                move |taken, cx| {
                    this.update(cx, |n, cx| {
                        n.made(taken.is_some(), name.clone(), window, cx)
                    })
                    .ok();
                },
            )
        });
    }

    /// Made: it goes on the end of the device's filters, a write of its
    /// own, so a failure there is said as that and the EQ is kept; and it
    /// opens to edit, in the window it was made from.
    fn made(
        &mut self,
        taken: bool,
        name: String,
        window: koan_kit::app::AnyWindowHandle,
        cx: &mut Context<Self>,
    ) {
        self.saving = false;
        if !taken {
            self.refusal = self.dsp.read(cx).last_error.clone();
            cx.notify();
            return;
        }
        let (dsp, sheets, device) = (self.dsp.clone(), self.sheets.clone(), self.device.clone());
        if let Some(device) = device.clone() {
            let added = name.clone();
            dsp.update(cx, |d, cx| {
                d.act(cx, move |e| async move {
                    let mut chain = e.clone().dsp_overview_for(Some(device.clone())).await.chain;
                    if !chain.iter().any(|entry| entry.name == added) {
                        chain.push(DspTuningEntry {
                            name: added,
                            on: true,
                        });
                    }
                    e.dsp_set_tunings(device, chain).await
                })
            });
        }
        let me = cx.entity_id();
        sheets.update(cx, |s, cx| s.close(me, cx));
        window
            .update(cx, |_, window, cx| {
                let view = cx.new(|cx| {
                    super::profile::ProfilePage::new(dsp, sheets.clone(), name, device, window, cx)
                });
                sheets.update(cx, |s, cx| s.view(view, cx));
            })
            .ok();
    }
}

impl Render for NewEq {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let this = cx.weak_entity();
        let sheets = self.sheets.clone();
        let empty = self.trimmed(cx).is_empty();
        let cancel = Button::new("new-cancel", ButtonKind::Standard)
            .label("Cancel")
            .on_click(move |_, _, cx| sheets.update(cx, |s, cx| s.pop(cx)));
        let create = Button::new("new-create", ButtonKind::Standard)
            .label("Create")
            .disabled(empty || self.saving)
            .on_click({
                let this = this.clone();
                move |_, window, cx| {
                    this.update(cx, |n, cx| n.create(window, cx)).ok();
                }
            });
        div()
            .flex()
            .flex_col()
            .child(head("New EQ", vec![cancel.into_any_element(), create.into_any_element()], &look))
            .child(
                body("new-body", &look)
                    .child(TextField::new(&self.name))
                    .child(FormRow::new("Start from").control(
                        Segmented::new("new-starter", STARTERS.map(|s| s.0), self.starter).on_change(move |i, _, cx| {
                            this.update(cx, |n, cx| {
                                n.starter = *i;
                                cx.notify();
                            })
                            .ok();
                        }),
                    ))
                    .child(note(
                        "A low shelf, six peaks an octave apart from 200 Hz, and a high shelf, each at 0 dB unless the starting curve says otherwise. Every band can be moved, retyped or removed.",
                        Tone::Muted,
                        &look,
                    ))
                    .when_some(self.refusal.clone(), |d, r| d.child(note(r, Tone::Bad, &look))),
            )
    }
}
