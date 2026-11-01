//! The ways a correction comes in besides a file: AutoEQ by headphone name,
//! a measurement corrected to a target, and a baked EQ split into a
//! correction and a tuning. Each shows what it will make before it is made.

use std::path::PathBuf;
use std::time::Duration;

use koan_ffi::{
    AutoEqEntry, AutoEqOffer, DspEarKind, DspMeasurementReading, DspResponse, DspTargetOption,
    SquigHit,
};
use koan_kit::app::*;
// gpui-component's text input: koan-kit's TextField draws it, the page owns its state.
use koan_kit::raw::component::input::{InputEvent, InputState, Textarea, TextareaState};
// sizes and colours computed at runtime from measurements, not chosen from the theme.
use koan_kit::raw::px;

use super::graph::{Graph, Shown};
use super::model::{Dsp, describe};
use super::sheet::{Confirm, Sheets, body, choice_row, head, head_data, note, section};
use super::stage::target_label;
use crate::components::{Button, ButtonKind, EmptyState, FormRow, Segmented, TextField};
use crate::theme::{Icon, KoanIcon, KoanStyled, Look, Space, Tone, TypeRole, label};

/// The output in use, recognised by its name as a headphone AutoEQ has
/// measured, or roughly so: its profile, or a search for its model. Offered
/// quietly; nothing is applied until asked, and what Use does is said.
pub fn suggestion(
    offer: AutoEqOffer,
    dsp: Entity<Dsp>,
    sheets: Entity<Sheets>,
    look: &Look,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let dismiss = {
        let dsp = dsp.clone();
        Button::new("suggest-dismiss", ButtonKind::Link)
            .label("Not for This Device")
            .on_click(move |_, _, cx| dsp.update(cx, |d, cx| d.dismiss_suggestion(cx)))
    };
    let device = dsp
        .read(cx)
        .overview
        .as_ref()
        .and_then(|o| o.device.clone());
    let device = device
        .map(|d| dsp.read(cx).label(&d))
        .unwrap_or_else(|| "the output in use".into());
    match offer {
        AutoEqOffer::Profile { entry } => {
            let use_it = {
                let (dsp, sheets, entry) = (dsp.clone(), sheets.clone(), entry.clone());
                Button::new("suggest-use", ButtonKind::Prominent)
                    .label("Use")
                    .on_click(move |_, _, cx| {
                        install(&dsp, &sheets, entry.clone(), &device, cx);
                    })
            };
            div()
                .flex()
                .flex_col()
                .gap(look.space(Space::S))
                .child(note(
                    format!("AutoEQ has a correction for {}. Use it?", entry.name),
                    Tone::Muted,
                    look,
                ))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(look.space(Space::M))
                        .child(use_it)
                        .child(dismiss),
                )
                .into_any_element()
        }
        AutoEqOffer::Search { query } => {
            let find = {
                let (dsp, sheets, query) = (dsp.clone(), sheets.clone(), query.clone());
                Button::new("suggest-find", ButtonKind::Bordered)
                    .text(format!("{} {query} {}", label("Find"), label("in AutoEQ…")))
                    .on_click(move |_, window, cx| {
                        let view = cx.new(|cx| {
                            AutoEqSearch::new(
                                dsp.clone(),
                                sheets.clone(),
                                query.clone(),
                                window,
                                cx,
                            )
                        });
                        sheets.update(cx, |s, cx| s.view(view, cx));
                    })
            };
            div()
                .flex()
                .items_center()
                .gap(look.space(Space::M))
                .child(find)
                .child(dismiss)
                .into_any_element()
        }
    }
}

/// Installing a result downloads it and makes it the output's correction,
/// which is said first.
fn install(
    dsp: &Entity<Dsp>,
    sheets: &Entity<Sheets>,
    entry: AutoEqEntry,
    device: &str,
    cx: &mut App,
) {
    let current = dsp
        .read(cx)
        .overview
        .as_ref()
        .and_then(|o| o.active.clone());
    let replaces = current
        .map(|c| format!(" in place of {c}"))
        .unwrap_or_default();
    let dsp = dsp.clone();
    sheets.update(cx, |s, cx| {
        s.confirm(
            Confirm {
                title: format!("Use {}?", entry.name).into(),
                message: format!(
                    "AutoEQ's correction for {}, measured by {}, is downloaded and becomes {device}'s correction{replaces}.",
                    entry.name, entry.measured_by
                )
                .into(),
                action: "Use".into(),
                destructive: false,
                also: None,
                run: std::rc::Rc::new(move |_, cx| {
                    let entry = entry.clone();
                    dsp.update(cx, |d, cx| d.install_autoeq(entry, cx));
                }),
            },
            cx,
        )
    });
}

/// AutoEQ's results by headphone name; with nothing typed, its makers, each
/// opening its models.
pub struct AutoEqSearch {
    dsp: Entity<Dsp>,
    sheets: Entity<Sheets>,
    query: Entity<InputState>,
    /// A maker opened, and its models once read.
    maker: Option<(String, Vec<AutoEqEntry>)>,
    /// Why the makers or a maker's models could not be read.
    problem: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl AutoEqSearch {
    pub fn new(
        dsp: Entity<Dsp>,
        sheets: Entity<Sheets>,
        query: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(label("Headphone"))
                .default_value(query.clone())
        });
        let subscriptions = vec![
            cx.subscribe_in(
                &input,
                window,
                |s: &mut Self, input, e: &InputEvent, _, cx| {
                    if let InputEvent::Change = e {
                        let q = input.read(cx).value().to_string();
                        s.maker = None;
                        s.dsp.update(cx, |d, cx| d.search_autoeq(q, cx));
                    }
                },
            ),
            cx.observe(&dsp, |_, _, cx| cx.notify()),
        ];
        dsp.update(cx, |d, cx| {
            d.load_autoeq_makers(cx);
            d.search_autoeq(query, cx);
        });
        Self {
            dsp,
            sheets,
            query: input,
            maker: None,
            problem: None,
            _subscriptions: subscriptions,
        }
    }

    fn open_maker(&mut self, maker: String, cx: &mut Context<Self>) {
        self.maker = Some((maker.clone(), Vec::new()));
        cx.notify();
        let engine = self.dsp.read(cx).engine();
        cx.spawn(async move |this, cx| {
            let models = engine.autoeq_models(maker.clone()).await;
            this.update(cx, |s, cx| {
                if s.maker.as_ref().is_some_and(|(m, _)| *m == maker) {
                    match models {
                        Ok(models) => s.maker = Some((maker, models)),
                        Err(e) => s.problem = Some(describe(&e)),
                    }
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn entry_row(&self, i: usize, entry: &AutoEqEntry, look: &Look, cx: &App) -> AnyElement {
        let (dsp, sheets, e) = (self.dsp.clone(), self.sheets.clone(), entry.clone());
        let device = dsp
            .read(cx)
            .overview
            .as_ref()
            .and_then(|o| o.device.clone());
        let device = device
            .map(|d| dsp.read(cx).label(&d))
            .unwrap_or_else(|| "the output in use".into());
        choice_row(
            ("autoeq", i),
            entry.name.clone(),
            Some(entry.measured_by.clone().into()),
            None,
            look,
            move |_, _, cx| {
                sheets.update(cx, |s, cx| s.pop(cx));
                install(&dsp, &sheets, e.clone(), &device, cx);
            },
        )
        .into_any_element()
    }
}

impl Render for AutoEqSearch {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let sheets = self.sheets.clone();
        let cancel = Button::new("autoeq-cancel", ButtonKind::Standard)
            .label("Cancel")
            .on_click(move |_, _, cx| sheets.update(cx, |s, cx| s.pop(cx)));
        let query = self.query.read(cx).value().trim().to_owned();
        let d = self.dsp.read(cx);
        let mut rows: Vec<AnyElement> = Vec::new();
        if let Some((maker, models)) = &self.maker {
            let this = cx.weak_entity();
            rows.push(
                Button::new("autoeq-back", ButtonKind::Link)
                    .label("All makers")
                    .icon(Icon::Back)
                    .on_click(move |_, _, cx| {
                        this.update(cx, |s, cx| {
                            s.maker = None;
                            cx.notify();
                        })
                        .ok();
                    })
                    .into_any_element(),
            );
            rows.push(crate::components::SectionHeader::new(maker).into_any_element());
            for (i, e) in models.iter().enumerate() {
                rows.push(self.entry_row(i, e, &look, cx));
            }
        } else if query.is_empty() {
            for (i, m) in d.autoeq_makers.iter().enumerate() {
                let (this, name) = (cx.weak_entity(), m.name.clone());
                rows.push(
                    choice_row(
                        ("maker", i),
                        m.name.clone(),
                        None,
                        Some(note(format!("{}", m.results), Tone::Muted, &look).into_any_element()),
                        &look,
                        move |_, _, cx| {
                            this.update(cx, |s, cx| s.open_maker(name.clone(), cx)).ok();
                        },
                    )
                    .into_any_element(),
                );
            }
        } else if d.autoeq_results.is_empty() {
            let (dsp, sheets, q) = (self.dsp.clone(), self.sheets.clone(), query.clone());
            let search = cx.entity_id();
            rows.push(
                EmptyState::new(Icon::Search, "Not in AutoEQ")
                    .detail(format!("AutoEQ has nothing for “{query}”. A measurement of your headphones works too: kōan builds the correction from it."))
                    .into_any_element(),
            );
            rows.push(
                Button::new("autoeq-measure", ButtonKind::Bordered)
                    .label("Find a Measurement…")
                    .on_click(move |_, window, cx| {
                        let view = cx.new(|cx| {
                            MeasurementFlow::new(dsp.clone(), sheets.clone(), q.clone(), window, cx)
                                .closing(search)
                        });
                        sheets.update(cx, |s, cx| s.view(view, cx));
                    })
                    .into_any_element(),
            );
        } else {
            for (i, e) in d.autoeq_results.iter().enumerate() {
                rows.push(self.entry_row(i, e, &look, cx));
            }
        }
        div()
            .flex()
            .flex_col()
            .child(head("AutoEQ", vec![cancel.into_any_element()], &look))
            .child(
                body("autoeq-body", &look)
                    .child(TextField::new(&self.query))
                    .children(self.problem.clone().map(|p| note(p, Tone::Bad, &look)))
                    .children(rows),
            )
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Step {
    Learn,
    File,
    Ear,
    Target,
    Review,
}

impl Step {
    fn title(self) -> &'static str {
        match self {
            Step::Learn => "Find a measurement",
            Step::File => "Your measurement",
            Step::Ear => "In-ear or over-ear",
            Step::Target => "Choose a target",
            Step::Review => "Check and save",
        }
    }
}

/// Correcting headphones AutoEQ does not have, from a measurement of them:
/// what a measurement is and where to get one, the file, in-ear or
/// over-ear, a target, and what the correction does before it is saved.
pub struct MeasurementFlow {
    dsp: Entity<Dsp>,
    sheets: Entity<Sheets>,
    step: Step,
    text: Entity<TextareaState>,
    file: Option<String>,
    in_ear: bool,
    targets: Vec<DspTargetOption>,
    target: Option<String>,
    graph: Entity<Graph>,
    name: Entity<InputState>,
    problem: Option<SharedString>,
    reading: Option<DspMeasurementReading>,
    saving: bool,
    squig: Entity<InputState>,
    hits: Vec<SquigHit>,
    searching: bool,
    search_note: Option<SharedString>,
    picking: Option<SquigHit>,
    fetched: Option<SquigHit>,
    fetch_problem: Option<(SquigHit, SharedString)>,
    _search: Option<Task<()>>,
    /// The step's check or a result's fetch under way: Back, or choosing
    /// something else, drops it.
    _step: Option<Task<()>>,
    _fetch: Option<Task<()>>,
    /// A sheet this one was opened from, closed with it once a correction
    /// is saved: AutoEQ's search, which found nothing.
    closing: Option<koan_kit::app::EntityId>,
    _subscriptions: Vec<Subscription>,
}

impl MeasurementFlow {
    pub fn closing(mut self, sheet: koan_kit::app::EntityId) -> Self {
        self.closing = Some(sheet);
        self
    }

    pub fn new(
        dsp: Entity<Dsp>,
        sheets: Entity<Sheets>,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let text = cx.new(|cx| TextareaState::new(window, cx).rows(6));
        let name_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(label("Name"))
                .default_value(name.clone())
        });
        let squig = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(label("Search…"))
                .default_value(name.clone())
        });
        let graph =
            cx.new(|cx| Graph::new(Graph::flat_response(), window, cx).open_on(Shown::Measured));
        let subscriptions = vec![
            cx.subscribe_in(&squig, window, |m: &mut Self, _, e: &InputEvent, _, cx| {
                if let InputEvent::Change = e {
                    m.search(cx);
                }
            }),
            cx.observe(&text, |_, _, cx| cx.notify()),
            cx.observe(&name_input, |_, _, cx| cx.notify()),
        ];
        Self {
            dsp,
            sheets,
            step: Step::Learn,
            text,
            file: None,
            in_ear: true,
            targets: Vec::new(),
            target: None,
            graph,
            name: name_input,
            problem: None,
            reading: None,
            saving: false,
            squig,
            hits: Vec::new(),
            searching: false,
            search_note: None,
            picking: None,
            fetched: None,
            fetch_problem: None,
            _search: None,
            _step: None,
            _fetch: None,
            closing: None,
            _subscriptions: subscriptions,
        }
    }

    fn text(&self, cx: &App) -> String {
        self.text.read(cx).value().to_string()
    }

    /// A speaker's measurement: corrected to Flat, the one speaker target.
    fn speaker(&self) -> bool {
        self.reading.as_ref().is_some_and(|r| r.speaker)
    }

    fn can_go_on(&self, cx: &App) -> bool {
        match self.step {
            Step::Learn | Step::Ear => true,
            Step::File => !self.text(cx).trim().is_empty(),
            Step::Target => self.target.is_some(),
            Step::Review => false,
        }
    }

    /// Searches squig.link's sites once typing pauses; a search overtaken by
    /// newer typing is dropped with its task.
    fn search(&mut self, cx: &mut Context<Self>) {
        let query = self.squig.read(cx).value().trim().to_owned();
        if query.is_empty() {
            self._search = None;
            self.hits.clear();
            self.search_note = None;
            self.searching = false;
            cx.notify();
            return;
        }
        let engine = self.dsp.read(cx).engine();
        self._search = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(300))
                .await;
            this.update(cx, |m, cx| {
                m.searching = true;
                cx.notify();
            })
            .ok();
            let found = engine.dsp_squig_search(query.clone()).await;
            this.update(cx, |m, cx| {
                m.searching = false;
                match found {
                    Ok(hits) => {
                        m.search_note = hits
                            .is_empty()
                            .then(|| format!("Nothing on squig.link matches “{query}”.").into());
                        m.hits = hits;
                    }
                    Err(e) => {
                        m.hits.clear();
                        m.search_note = Some(describe(&e));
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn pick(&mut self, hit: SquigHit, window: &mut Window, cx: &mut Context<Self>) {
        self.picking = Some(hit.clone());
        self.fetch_problem = None;
        cx.notify();
        let engine = self.dsp.read(cx).engine();
        self._fetch = Some(cx.spawn_in(window, async move |this, cx| {
            let fetched = engine
                .dsp_squig_fetch(hit.site.clone(), hit.file.clone())
                .await;
            this.update_in(cx, |m, window, cx| {
                m.picking = None;
                match fetched {
                    Ok(text) => {
                        m.text.update(cx, |t, cx| t.set_value(text, window, cx));
                        m.file = Some(format!("{}, {}", hit.name, hit.site_label));
                        m.problem = None;
                        if m.name.read(cx).value().trim().is_empty() {
                            let name = format!("{} {}", hit.brand, hit.model);
                            m.name.update(cx, |n, cx| n.set_value(name, window, cx));
                        }
                        if let Some(in_ear) = hit.in_ear {
                            m.in_ear = in_ear;
                        }
                        m.fetched = Some(hit);
                    }
                    Err(e) => m.fetch_problem = Some((hit, describe(&e))),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Reads the chosen files as one text, off the main thread: a speaker's
    /// two planes are read together, and one plane of an Audio Science
    /// Review export brings its sibling along where it can be read.
    fn choose_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A file chosen wins over a result still being fetched.
        self._fetch = None;
        self.picking = None;
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            if paths.is_empty() {
                return;
            }
            let read = cx
                .background_executor()
                .spawn(async move { read_planes(paths) })
                .await;
            this.update_in(cx, |m, window, cx| {
                match read {
                    Ok((text, names, suggested)) => {
                        m.text.update(cx, |t, cx| t.set_value(text, window, cx));
                        m.file = Some(names.join(", "));
                        m.fetched = None;
                        m.problem = None;
                        if m.name.read(cx).value().trim().is_empty() {
                            m.name
                                .update(cx, |n, cx| n.set_value(suggested, window, cx));
                        }
                    }
                    Err(problem) => m.problem = Some(problem.into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn back(&mut self, cx: &mut Context<Self>) {
        self._step = None;
        self.problem = None;
        self.step = match self.step {
            Step::Learn | Step::File => Step::Learn,
            Step::Ear => Step::File,
            // A speaker has no in-ear or over-ear to ask.
            Step::Target if self.speaker() => Step::File,
            Step::Target => Step::Ear,
            Step::Review => Step::Target,
        };
        cx.notify();
    }

    fn next(&mut self, cx: &mut Context<Self>) {
        self.problem = None;
        let engine = self.dsp.read(cx).engine();
        let (step, text, in_ear, target) =
            (self.step, self.text(cx), self.in_ear, self.target.clone());
        self._step = Some(cx.spawn(async move |this, cx| {
            match step {
                Step::Learn => {
                    this.update(cx, |m, cx| m.go(Step::File, cx)).ok();
                }
                Step::File => {
                    // Checked now, so a file that is not a measurement is
                    // said so before the questions after it.
                    let reading = engine.clone().dsp_describe_measurement(text).await;
                    let speaker = reading.as_ref().is_ok_and(|r| r.speaker);
                    let targets = if speaker {
                        Some(engine.dsp_targets_for(DspEarKind::Speaker).await)
                    } else {
                        None
                    };
                    this.update(cx, |m, cx| match reading {
                        Ok(r) => {
                            m.reading = Some(r);
                            if let Some(targets) = targets {
                                m.target = targets
                                    .iter()
                                    .find(|t| t.id == "flat")
                                    .or(targets.first())
                                    .map(|t| t.id.clone());
                                m.targets = targets;
                                m.go(Step::Target, cx);
                            } else {
                                m.go(Step::Ear, cx);
                            }
                        }
                        Err(e) => {
                            m.problem = Some(describe(&e));
                            cx.notify();
                        }
                    })
                    .ok();
                }
                Step::Ear => {
                    let ear = if in_ear {
                        DspEarKind::InEar
                    } else {
                        DspEarKind::OverEar
                    };
                    let targets = engine.dsp_targets_for(ear).await;
                    this.update(cx, |m, cx| {
                        // Harman, as AutoEQ's corrections are, unless one was
                        // picked: what most people want.
                        if !targets.iter().any(|t| Some(&t.id) == m.target.as_ref()) {
                            let harman = if in_ear {
                                "harman-in-ear-2019"
                            } else {
                                "harman-over-ear-2018"
                            };
                            m.target = targets
                                .iter()
                                .find(|t| t.id == harman)
                                .or(targets.first())
                                .map(|t| t.id.clone());
                        }
                        m.targets = targets;
                        m.go(Step::Target, cx);
                    })
                    .ok();
                }
                Step::Target => {
                    let Some(target) = target else { return };
                    let preview = engine.dsp_preview_measurement(text, target).await;
                    this.update(cx, |m, cx| match preview {
                        Ok(r) => {
                            let stamp = m.dsp.read(cx).stamp();
                            m.graph.update(cx, |g, cx| g.set_response(r, stamp, cx));
                            m.go(Step::Review, cx);
                        }
                        Err(e) => {
                            m.problem = Some(describe(&e));
                            cx.notify();
                        }
                    })
                    .ok();
                }
                Step::Review => {}
            }
        }));
    }

    fn go(&mut self, step: Step, cx: &mut Context<Self>) {
        self.step = step;
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.target.clone() else {
            return;
        };
        let name = self.name.read(cx).value().trim().to_owned();
        if name.is_empty() || self.saving {
            return;
        }
        self.saving = true;
        let ear = if self.speaker() {
            DspEarKind::Speaker
        } else if self.in_ear {
            DspEarKind::InEar
        } else {
            DspEarKind::OverEar
        };
        let (text, source) = (
            self.text(cx),
            self.fetched.as_ref().map(|h| h.source.clone()),
        );
        let this = cx.weak_entity();
        let sheets = self.sheets.clone();
        let (me, closing) = (cx.entity_id(), self.closing);
        self.dsp.update(cx, |d, cx| {
            d.act_then(
                cx,
                move |e| e.dsp_save_measured(name, text, ear, target, source),
                move |saved, cx| {
                    if saved.is_some() {
                        sheets.update(cx, |s, cx| {
                            s.close(me, cx);
                            if let Some(from) = closing {
                                s.close(from, cx);
                            }
                        });
                    } else {
                        this.update(cx, |m, cx| {
                            m.saving = false;
                            m.problem = m.dsp.read(cx).last_error.clone();
                            cx.notify();
                        })
                        .ok();
                    }
                },
            )
        });
    }

    fn step_body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let look = Look::of(window, cx);
        let text = |s: &str| {
            div()
                .text_role(TypeRole::Body, &look)
                .text_color(look.palette.ink.hsla())
                .child(s.to_owned())
                .into_any_element()
        };
        match self.step {
            Step::Learn => vec![
                text("A measurement is a graph of how loud your headphones play each pitch, from deep bass to high treble. People who test headphones publish them."),
                text("You choose a target, the sound you want. kōan works out a correction: what to turn up and down so your headphones sound like the target."),
                section(
                    Some("Where to get one"),
                    vec![
                        Button::new("learn-squig", ButtonKind::Link).label("squig.link").on_click(|_, _, cx| cx.open_url("https://squig.link")).into_any_element(),
                        note("Search for your headphones on the next page: kōan looks through the reviewers' squig.link sites. Or save their frequency response as a file from one. REW and most measurement tools export one too.", Tone::Muted, &look).into_any_element(),
                    ],
                    None,
                    &look,
                )
                .into_any_element(),
                section(
                    Some("What the file looks like"),
                    vec![div().text_role(TypeRole::Meta, &look).text_color(look.text_color(Tone::Muted)).child("20, 92.4  ·  21, 92.6  ·  22, 92.7  ·  …").into_any_element()],
                    Some("Two numbers a line: a frequency in hertz, then a level in decibels. Commas, tabs or spaces between them all work.".into()),
                    &look,
                )
                .into_any_element(),
                Button::new("learn-more", ButtonKind::Link)
                    .label("Learn more about headphone EQ")
                    .on_click(|_, _, cx| cx.open_url("https://koan.rocks/docs/headphone-eq/"))
                    .into_any_element(),
            ],
            Step::File => {
                let mut rows: Vec<AnyElement> = vec![TextField::new(&self.squig).into_any_element()];
                if self.searching && self.hits.is_empty() {
                    rows.push(note("Searching squig.link…", Tone::Muted, &look).into_any_element());
                } else if let Some(n) = &self.search_note {
                    rows.push(note(n.clone(), Tone::Muted, &look).into_any_element());
                }
                for (i, hit) in self.hits.iter().take(20).enumerate() {
                    let details = [Some(hit.site_label.clone()), hit.rig.clone().map(|r| format!("{r} rig"))]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" · ");
                    if hit.locked.is_some() {
                        // Fetching would only fail: the site is offered instead.
                        let site = hit.site.clone();
                        rows.push(
                            choice_row(("squig", i), hit.name.clone(), Some(format!("{details} · opens in a browser").into()), None, &look, move |_, _, cx| cx.open_url(&site))
                                .into_any_element(),
                        );
                        continue;
                    }
                    let mark = if self.picking.as_ref() == Some(hit) {
                        Some(note("…", Tone::Muted, &look).into_any_element())
                    } else if self.fetched.as_ref() == Some(hit) {
                        Some(KoanIcon::new(Icon::Check).role(TypeRole::Body).tone(Tone::Accent).into_any_element())
                    } else {
                        None
                    };
                    let problem = self.fetch_problem.as_ref().filter(|(h, _)| h == hit).map(|(_, m)| m.clone());
                    let (this, h) = (cx.weak_entity(), hit.clone());
                    let busy = self.picking.is_some();
                    rows.push(
                        div()
                            .flex()
                            .flex_col()
                            .child(choice_row(("squig", i), hit.name.clone(), Some(details.into()), mark, &look, move |_, window, cx| {
                                if !busy {
                                    this.update(cx, |m, cx| m.pick(h.clone(), window, cx)).ok();
                                }
                            }))
                            .children(problem.map(|p| note(p, Tone::Bad, &look)))
                            .into_any_element(),
                    );
                }
                if let Some(why) = self.hits.iter().take(20).find_map(|h| h.locked.clone()) {
                    rows.push(note(format!("{why}."), Tone::Muted, &look).into_any_element());
                }
                let this = cx.weak_entity();
                vec![
                    section(
                        Some("Find it on squig.link"),
                        rows,
                        Some("Measurements reviewers publish on their squig.link sites. Pick one measured on the rig your target assumes, where the site says, and to match a site's own EQ presets, its measurement; the site is credited on the correction.".into()),
                        &look,
                    )
                    .into_any_element(),
                    section(
                        None,
                        vec![
                            Button::new("measure-file", ButtonKind::Bordered)
                                .text(self.file.as_ref().map(|f| format!("{} {f}", label("Chosen:"))).unwrap_or_else(|| label("Choose a File…").to_string()))
                                .on_click(move |_, window, cx| {
                                    this.update(cx, |m, cx| m.choose_files(window, cx)).ok();
                                })
                                .into_any_element(),
                        ],
                        Some("A .csv or .txt file of frequency and level. For a speaker measured by Audio Science Review, choose both its SPL Horizontal and SPL Vertical files.".into()),
                        &look,
                    )
                    .into_any_element(),
                    section(
                        Some("Or paste it here"),
                        vec![div().p(look.space(Space::S)).bg(look.palette.surface.hsla()).child(Textarea::new(&self.text).appearance(false)).into_any_element()],
                        None,
                        &look,
                    )
                    .into_any_element(),
                ]
            }
            Step::Ear => {
                let this = cx.weak_entity();
                vec![
                    FormRow::new("Your headphones are")
                        .control(Segmented::new("measure-ear", ["In-ear", "Over-ear"], (!self.in_ear) as usize).on_change(move |i, _, cx| {
                            this.update(cx, |m, cx| {
                                m.in_ear = *i == 0;
                                cx.notify();
                            })
                            .ok();
                        }))
                        .into_any_element(),
                    note(
                        if self.in_ear {
                            "In-ear: earphones and IEMs that sit in your ear canal."
                        } else {
                            "Over-ear: headphones with cups that sit around or on your ears."
                        },
                        Tone::Muted,
                        &look,
                    )
                    .into_any_element(),
                ]
            }
            Step::Target => {
                let mut rows: Vec<AnyElement> = Vec::new();
                if self.speaker()
                    && let Some(r) = &self.reading
                {
                    rows.push(section(Some("Read as"), vec![note(r.note.clone(), Tone::Muted, &look).into_any_element()], None, &look).into_any_element());
                }
                let choices = self
                    .targets
                    .iter()
                    .enumerate()
                    .map(|(i, t)| {
                        let (this, id) = (cx.weak_entity(), t.id.clone());
                        let chosen = self.target.as_ref() == Some(&t.id);
                        choice_row(
                            ("target", i),
                            target_label(t, false),
                            (!t.character.is_empty()).then(|| t.character.clone().into()),
                            chosen.then(|| KoanIcon::new(Icon::Check).role(TypeRole::Body).tone(Tone::Accent).into_any_element()),
                            &look,
                            move |_, _, cx| {
                                this.update(cx, |m, cx| {
                                    m.target = Some(id.clone());
                                    cx.notify();
                                })
                                .ok();
                            },
                        )
                        .into_any_element()
                    })
                    .collect();
                rows.push(
                    section(
                        Some("Target"),
                        choices,
                        Some("Each target is a different idea of a good sound. You can change it later on the correction's page.".into()),
                        &look,
                    )
                    .into_any_element(),
                );
                rows
            }
            Step::Review => {
                let what = if self.speaker() { "speaker" } else { "headphones" };
                vec![
                    self.graph.clone().into_any_element(),
                    note(
                        format!("Measured shows your measurement, the target, and what your {what} will sound like with the correction. EQ shows the correction itself."),
                        Tone::Muted,
                        &look,
                    )
                    .into_any_element(),
                    TextField::new(&self.name).into_any_element(),
                    note(
                        format!(
                            "Usually the {}' name. It becomes a correction: add tunings, like more bass or a room tilt, on top of it.",
                            if self.speaker() { "speaker" } else { "headphones" }
                        ),
                        Tone::Muted,
                        &look,
                    )
                    .into_any_element(),
                ]
            }
        }
    }
}

/// The files' text, their names, and the name to suggest: an Audio Science
/// Review export's folder, otherwise the first file's.
fn read_planes(paths: Vec<PathBuf>) -> Result<(String, Vec<String>, String), String> {
    let mut texts = Vec::new();
    let mut names = Vec::new();
    let file_name = |p: &PathBuf| {
        p.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    for p in &paths {
        let text = std::fs::read_to_string(p)
            .map_err(|_| format!("{} could not be read as text.", file_name(p)))?;
        texts.push(text);
        names.push(file_name(p));
    }
    let other = |p: &PathBuf| -> Option<PathBuf> {
        let sibling = match file_name(p).as_str() {
            "SPL Horizontal.txt" => "SPL Vertical.txt",
            "SPL Vertical.txt" => "SPL Horizontal.txt",
            _ => return None,
        };
        p.parent().map(|d| d.join(sibling))
    };
    // Best effort: without the other plane the measurement reads as on-axis,
    // and the note asks for it.
    if paths.len() == 1
        && let Some(sibling) = other(&paths[0])
        && let Ok(text) = std::fs::read_to_string(&sibling)
    {
        texts.push(text);
        names.push(file_name(&sibling));
    }
    let first = &paths[0];
    let suggested = if other(first).is_some() {
        first
            .parent()
            .and_then(|d| d.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    } else {
        first
            .file_stem()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    Ok((texts.join("\n"), names, suggested))
}

impl Render for MeasurementFlow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let this = cx.weak_entity();
        let sheets = self.sheets.clone();
        let mut actions = vec![
            Button::new("measure-cancel", ButtonKind::Standard)
                .label("Cancel")
                .disabled(self.saving)
                .on_click({
                    let me = cx.entity_id();
                    move |_, _, cx| sheets.update(cx, |s, cx| s.close(me, cx))
                })
                .into_any_element(),
        ];
        if self.step != Step::Learn {
            let this = this.clone();
            actions.push(
                Button::new("measure-back", ButtonKind::Standard)
                    .label("Back")
                    .on_click(move |_, _, cx| {
                        this.update(cx, |m, cx| m.back(cx)).ok();
                    })
                    .into_any_element(),
            );
        }
        if self.step == Step::Review {
            let empty = self.name.read(cx).value().trim().is_empty();
            actions.push(
                Button::new("measure-save", ButtonKind::Standard)
                    .label("Save")
                    .disabled(empty || self.saving)
                    .on_click(move |_, _, cx| {
                        this.update(cx, |m, cx| m.save(cx)).ok();
                    })
                    .into_any_element(),
            );
        } else {
            let can = self.can_go_on(cx);
            actions.push(
                Button::new("measure-next", ButtonKind::Standard)
                    .label("Next")
                    .disabled(!can)
                    .on_click(move |_, _, cx| {
                        this.update(cx, |m, cx| m.next(cx)).ok();
                    })
                    .into_any_element(),
            );
        }
        let steps = self.step_body(window, cx);
        div()
            .flex()
            .flex_col()
            .child(head(self.step.title(), actions, &look))
            .child(
                body("measure-body", &look)
                    .children(steps)
                    .when_some(self.problem.clone(), |d, p| {
                        d.child(note(p, Tone::Bad, &look))
                    }),
            )
    }
}

/// Taking a baked EQ apart: with a measurement of the headphones and the
/// target that counts as neutral, the correction is target minus
/// measurement, and the tuning is what the EQ does beyond it. Both are drawn
/// against the EQ before anything is saved.
pub struct SplitFlow {
    dsp: Entity<Dsp>,
    sheets: Entity<Sheets>,
    name: String,
    text: Option<String>,
    file: Option<String>,
    in_ear: bool,
    targets: Vec<DspTargetOption>,
    target: Option<String>,
    graph: Entity<Graph>,
    preview: Option<DspResponse>,
    problem: Option<SharedString>,
    saving: bool,
    _refresh: Option<Task<()>>,
}

impl SplitFlow {
    pub fn new(
        dsp: Entity<Dsp>,
        sheets: Entity<Sheets>,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let graph = cx.new(|cx| Graph::new(Graph::flat_response(), window, cx));
        let mut flow = Self {
            dsp,
            sheets,
            name,
            text: None,
            file: None,
            in_ear: true,
            targets: Vec::new(),
            target: None,
            graph,
            preview: None,
            problem: None,
            saving: false,
            _refresh: None,
        };
        flow.load_targets(cx);
        flow
    }

    fn load_targets(&mut self, cx: &mut Context<Self>) {
        self.preview = None;
        self._refresh = None;
        let engine = self.dsp.read(cx).engine();
        let in_ear = self.in_ear;
        cx.spawn(async move |this, cx| {
            let targets = engine
                .dsp_targets_for(if in_ear {
                    DspEarKind::InEar
                } else {
                    DspEarKind::OverEar
                })
                .await;
            this.update(cx, |f, cx| {
                if f.in_ear != in_ear {
                    return;
                }
                if !targets.iter().any(|t| Some(&t.id) == f.target.as_ref()) {
                    let harman = if in_ear {
                        "harman-in-ear-2019"
                    } else {
                        "harman-over-ear-2018"
                    };
                    f.target = targets
                        .iter()
                        .find(|t| t.id == harman)
                        .or(targets.first())
                        .map(|t| t.id.clone());
                }
                f.targets = targets;
                f.refresh(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Draws what the split would give, each time the measurement or the
    /// target changes; a newer one replaces the task under way.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let (Some(text), Some(target)) = (self.text.clone(), self.target.clone()) else {
            cx.notify();
            return;
        };
        // What was drawn is for the last measurement or target: Split waits
        // for this one.
        self.preview = None;
        let engine = self.dsp.read(cx).engine();
        let name = self.name.clone();
        self._refresh = Some(cx.spawn(async move |this, cx| {
            let preview = engine.dsp_preview_split(name, text, target).await;
            this.update(cx, |f, cx| {
                match preview {
                    Ok(r) => {
                        let stamp = f.dsp.read(cx).stamp();
                        f.graph
                            .update(cx, |g, cx| g.set_response(r.clone(), stamp, cx));
                        f.preview = Some(r);
                        f.problem = None;
                    }
                    Err(e) => {
                        f.preview = None;
                        f.problem = Some(describe(&e));
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn choose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let file = path.file_name().map(|n| n.to_string_lossy().into_owned());
            let text = cx
                .background_executor()
                .spawn(async move { std::fs::read_to_string(path) })
                .await;
            this.update(cx, |f, cx| match text {
                Ok(text) => {
                    f.text = Some(text);
                    f.file = file;
                    f.refresh(cx);
                }
                Err(_) => {
                    f.problem = Some("That file could not be read as text.".into());
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn split(&mut self, cx: &mut Context<Self>) {
        let (Some(text), Some(target)) = (self.text.clone(), self.target.clone()) else {
            return;
        };
        if self.saving || self.preview.is_none() {
            return;
        }
        self.saving = true;
        let (name, in_ear) = (self.name.clone(), self.in_ear);
        let this = cx.weak_entity();
        let sheets = self.sheets.clone();
        let me = cx.entity_id();
        self.dsp.update(cx, |d, cx| {
            d.act_then(
                cx,
                move |e| e.dsp_split_baked(name, text, in_ear, target),
                move |made, cx| {
                    if made.is_some() {
                        sheets.update(cx, |s, cx| s.close(me, cx));
                    } else {
                        this.update(cx, |f, cx| {
                            f.saving = false;
                            f.problem = f.dsp.read(cx).last_error.clone();
                            cx.notify();
                        })
                        .ok();
                    }
                },
            )
        });
    }
}

impl Render for SplitFlow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let this = cx.weak_entity();
        let sheets = self.sheets.clone();
        let name = self.name.clone();
        let me = cx.entity_id();
        let cancel = Button::new("split-cancel", ButtonKind::Standard)
            .label("Cancel")
            .disabled(self.saving)
            .on_click(move |_, _, cx| sheets.update(cx, |s, cx| s.close(me, cx)));
        let go = Button::new("split-go", ButtonKind::Standard)
            .label("Split")
            .disabled(self.preview.is_none() || self.saving)
            .on_click({
                let this = this.clone();
                move |_, _, cx| {
                    this.update(cx, |f, cx| f.split(cx)).ok();
                }
            });
        let mut rows: Vec<AnyElement> = vec![
            div()
                .text_role(TypeRole::Body, &look)
                .text_color(look.palette.ink.hsla())
                .child(format!("{name} is a correction for a device that already includes a tuning. With a measurement of the device, kōan takes it apart: a correction that makes it neutral, and the tuning, your taste, which then works on any device."))
                .into_any_element(),
        ];
        {
            let (choose, ear) = (this.clone(), this.clone());
            rows.push(
                section(
                    Some("Measurement"),
                    vec![
                        Button::new("split-file", ButtonKind::Bordered)
                            .text(
                                self.file.clone().unwrap_or_else(|| {
                                    label("Choose a Measurement File…").to_string()
                                }),
                            )
                            .on_click(move |_, window, cx| {
                                choose.update(cx, |f, cx| f.choose(window, cx)).ok();
                            })
                            .into_any_element(),
                        FormRow::new("Headphones")
                            .control(
                                Segmented::new(
                                    "split-ear",
                                    ["In-ear", "Over-ear"],
                                    (!self.in_ear) as usize,
                                )
                                .on_change(move |i, _, cx| {
                                    ear.update(cx, |f, cx| {
                                        f.in_ear = *i == 0;
                                        f.load_targets(cx);
                                    })
                                    .ok();
                                }),
                            )
                            .into_any_element(),
                    ],
                    Some("A frequency response of the device, from squig.link or REW.".into()),
                    &look,
                )
                .into_any_element(),
            );
        }
        if !self.targets.is_empty() {
            let choices = self
                .targets
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    let (this, id) = (this.clone(), t.id.clone());
                    let chosen = self.target.as_ref() == Some(&t.id);
                    choice_row(
                        ("split-target", i),
                        target_label(t, false),
                        None,
                        chosen.then(|| {
                            KoanIcon::new(Icon::Check)
                                .role(TypeRole::Body)
                                .tone(Tone::Accent)
                                .into_any_element()
                        }),
                        &look,
                        move |_, _, cx| {
                            this.update(cx, |f, cx| {
                                f.target = Some(id.clone());
                                f.refresh(cx);
                            })
                            .ok();
                        },
                    )
                    .into_any_element()
                })
                .collect();
            rows.push(
                section(
                    Some("Neutral is"),
                    choices,
                    Some(format!("The target {name} was made for. Harman is the usual one; the tuning is what it does beyond it.").into()),
                    &look,
                )
                .into_any_element(),
            );
        }
        if self.preview.is_some() {
            rows.push(self.graph.clone().into_any_element());
            rows.push(
                note(
                    format!("Correction and tuning together play as {name} does, but for the treble a correction holds back, where measurements disagree."),
                    Tone::Muted,
                    &look,
                )
                .into_any_element(),
            );
        }
        if let Some(p) = &self.problem {
            rows.push(note(p.clone(), Tone::Bad, &look).into_any_element());
        }
        rows.push(
            note(
                format!("Split makes “{name} correction” and “{name} tuning”. The outputs that play {name} will play the correction with the tuning on top, and {name} itself is kept."),
                Tone::Muted,
                &look,
            )
            .into_any_element(),
        );
        div()
            .flex()
            .flex_col()
            .child(head_data(
                format!("{} {name}", label("Split")),
                vec![cancel.into_any_element(), go.into_any_element()],
                &look,
            ))
            .child(body("split-body", &look).children(rows))
            .child(div().h(px(0.0)))
    }
}
