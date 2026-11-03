//! Organise: moving a pick's files to where a format pattern puts them,
//! under one of the library's folders.
//!
//! Music files are irreplaceable, so nothing moves until every destination
//! has been seen, the blocked ones included, which is the half a preview
//! usually leaves out. The selection is read once as the sheet opens
//! (`organize_selection`, the database and a `stat` per file); a pattern is
//! then generated against it as it is typed, touching no files, and checked
//! against the disk (destinations taken, what travels with each file) once
//! the typing settles. A newer preview replaces one in flight. Organising
//! asks first, with the counts, then moves the files and reports how it went.

use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use koan_kit::app::*;
use koan_runtime::{OrganizeEntry, OrganizePattern, OrganizePlan, OrganizeSelection, PlanOutcome};

use crate::components::sheet as ask;
use crate::components::{
    Button, ButtonKind, FieldState, FormRow, InputEvent, Picker, RowKind, TableRow, TextField,
    Toggle, table_list,
};
use crate::engine::engine;
use crate::paint::Paint;
use crate::theme::{KoanStyled, Tone, TypeRole};

/// How long typing rests before the plan is checked against the disk.
const SETTLE: Duration = Duration::from_millis(300);
/// The outcome column, in points.
const OUTCOME: f32 = 160.0;

/// Opens the sheet for `track_ids`, `title` naming them ("3 albums").
pub fn open(title: String, track_ids: Vec<i64>, window: &mut Window, cx: &mut App) {
    let sheet = cx.new(|cx| OrganizeSheet::new(title, track_ids, window, cx));
    ask::present(sheet, window, cx);
}

pub struct OrganizeSheet {
    title: String,
    track_ids: Vec<i64>,
    patterns: Vec<OrganizePattern>,
    /// The pattern chosen by name; `None` is a pattern typed for this once.
    chosen: Option<usize>,
    draft: Entity<FieldState>,
    /// The name the typed pattern is saved under.
    save_name: Entity<FieldState>,
    /// What the last save had to say, until the pattern changes again.
    saved: Option<String>,
    folders: Vec<String>,
    folder: usize,
    move_ancillary: bool,
    selection: Option<Arc<OrganizeSelection>>,
    plan: Option<OrganizePlan>,
    /// The plan shown has been checked against the disk.
    checked: bool,
    running: bool,
    error: Option<String>,
    /// Bumped per preview, so a slow plan cannot land on a newer one.
    generation: u64,
    /// A plan is being generated: the read pool runs it to the end even
    /// when it is no longer wanted, so one runs at a time, and the pattern
    /// typed meanwhile goes next (`stale`).
    generating: bool,
    stale: bool,
    _preview: Option<Task<()>>,
    _settle: Option<Task<()>>,
    /// The move under way. Its own task, so no preview replaces it.
    _run: Option<Task<()>>,
    _load: Task<()>,
    _subscriptions: Vec<Subscription>,
}

/// A format pattern's syntax, coloured: fields (`%album%`) in the accent,
/// functions (`$if2(`) strong, the brackets, commas and parentheses that
/// shape it and quoted literals muted; the rest is the path as written.
fn pattern_spans(pattern: &str) -> Vec<(Range<usize>, Tone)> {
    // Every mark is ASCII, which no byte of a longer character equals.
    let bytes = pattern.as_bytes();
    let closing = |from: usize, mark: u8| {
        bytes[from..]
            .iter()
            .position(|&b| b == mark)
            .map_or(bytes.len(), |i| from + i + 1)
    };
    let mut spans = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let (end, tone) = match bytes[at] {
            b'%' => (closing(at + 1, b'%'), Tone::Accent),
            b'\'' => (closing(at + 1, b'\''), Tone::Muted),
            b'$' => {
                let name = bytes[at + 1..]
                    .iter()
                    .position(|b| !(b.is_ascii_alphanumeric() || *b == b'_'))
                    .map_or(bytes.len(), |i| at + 1 + i);
                (
                    name + usize::from(bytes.get(name) == Some(&b'(')),
                    Tone::Strong,
                )
            }
            b'[' | b']' | b'(' | b')' | b',' => (at + 1, Tone::Muted),
            _ => {
                at += 1;
                continue;
            }
        };
        spans.push((at..end, tone));
        at = end;
    }
    spans
}

impl OrganizeSheet {
    fn new(
        title: String,
        track_ids: Vec<i64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let draft = cx.new(|cx| {
            FieldState::new(window, cx)
                .placeholder("%album artist%/…")
                .highlight(pattern_spans)
        });
        let save_name = cx.new(|cx| FieldState::new(window, cx).placeholder("name"));
        let subscriptions = vec![
            cx.subscribe(&draft, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.saved = None;
                    this.preview(cx);
                }
            }),
            cx.subscribe(&save_name, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
        ];
        let engine = engine(cx);
        let ids = track_ids.clone();
        let load = cx.spawn_in(window, async move |this, cx| {
            let patterns = engine.clone().organize_patterns().await;
            let folders = engine.clone().library_folders().await;
            let move_ancillary = engine.clone().organize_moves_ancillary().await;
            let selection = engine.clone().organize_selection(Some(ids)).await;
            this.update_in(cx, |this, window, cx| {
                let chosen = patterns
                    .iter()
                    .position(|p| p.is_default)
                    .or((!patterns.is_empty()).then_some(0));
                let text = chosen
                    .map(|i| patterns[i].pattern.clone())
                    .unwrap_or_default();
                this.patterns = patterns;
                this.chosen = chosen;
                this.folders = folders;
                this.move_ancillary = move_ancillary;
                match selection {
                    Ok(selection) => this.selection = Some(selection),
                    Err(e) => this.error = Some(crate::settings::model::describe(&e)),
                }
                // Setting the field says nothing to its subscription: the
                // preview is asked for here.
                this.draft.update(cx, |s, cx| s.set_value(text, window, cx));
                this.preview(cx);
            })
            .ok();
        });
        Self {
            title,
            track_ids,
            patterns: Vec::new(),
            chosen: None,
            draft,
            save_name,
            saved: None,
            folders: Vec::new(),
            folder: 0,
            move_ancillary: true,
            selection: None,
            plan: None,
            checked: false,
            running: false,
            error: None,
            generation: 0,
            generating: false,
            stale: false,
            _preview: None,
            _settle: None,
            _run: None,
            _load: load,
            _subscriptions: subscriptions,
        }
    }

    fn pattern(&self, cx: &App) -> String {
        self.draft.read(cx).value().trim().to_owned()
    }

    fn base(&self) -> Option<String> {
        self.folders.get(self.folder).cloned()
    }

    /// Generates the plan now, and checks it against the disk once typing
    /// has rested.
    fn preview(&mut self, cx: &mut Context<Self>) {
        // The plan being carried out stays on screen until it has been.
        if self.running {
            return;
        }
        let (Some(selection), Some(base)) = (self.selection.clone(), self.base()) else {
            return;
        };
        // A new plan replaces what the last move had to say.
        self.error = None;
        let pattern = self.pattern(cx);
        self.generation += 1;
        let generation = self.generation;
        self.checked = false;
        if pattern.is_empty() {
            self.plan = None;
            cx.notify();
            return;
        }
        self.generate(selection.clone(), pattern.clone(), base.clone(), cx);
        let ancillary = self.move_ancillary;
        self._settle = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SETTLE).await;
            let plan = selection.check(pattern, base, ancillary).await;
            this.update(cx, |this, cx| {
                if this.generation == generation {
                    this.plan = Some(plan);
                    this.checked = true;
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// The plan without the disk, as typed. One at a time: typing faster
    /// than plans are made generates only the newest pattern next.
    fn generate(
        &mut self,
        selection: Arc<OrganizeSelection>,
        pattern: String,
        base: String,
        cx: &mut Context<Self>,
    ) {
        if self.generating {
            self.stale = true;
            return;
        }
        self.generating = true;
        let generation = self.generation;
        let quick = selection.generate(pattern, base);
        self._preview = Some(cx.spawn(async move |this, cx| {
            let plan = quick.await;
            this.update(cx, |this, cx| {
                this.generating = false;
                // Not over the disk's answer, should that have come first.
                if this.generation == generation && !this.checked {
                    this.plan = Some(plan);
                    cx.notify();
                }
                if std::mem::take(&mut this.stale)
                    && !this.running
                    && !this.checked
                    && let (Some(selection), Some(base)) = (this.selection.clone(), this.base())
                {
                    let pattern = this.pattern(cx);
                    if !pattern.is_empty() {
                        this.generate(selection, pattern, base, cx);
                    }
                }
            })
            .ok();
        }));
    }

    /// Reads the selection again after a move that left some files behind,
    /// or one refused: the ones that moved are at new paths, which the next
    /// preview must start from, and a library that changed under the plan
    /// gives a new one. Previews again once it lands, the line saying what
    /// happened kept above the new plan.
    fn reread(&mut self, cx: &mut Context<Self>) {
        self.selection = None;
        let read = engine(cx).organize_selection(Some(self.track_ids.clone()));
        self._load = cx.spawn(async move |this, cx| {
            let selection = read.await;
            this.update(cx, |this, cx| {
                match selection {
                    Ok(selection) => {
                        this.selection = Some(selection);
                        let said = this.error.take();
                        this.preview(cx);
                        this.error = said;
                    }
                    Err(e) => this.error = Some(crate::settings::model::describe(&e)),
                }
                cx.notify();
            })
            .ok();
        });
    }

    fn choose_pattern(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pattern) = self.patterns.get(index) else {
            // "typed here": the field keeps what it holds.
            self.chosen = None;
            cx.notify();
            return;
        };
        self.chosen = Some(index);
        let text = pattern.pattern.clone();
        self.draft.update(cx, |s, cx| s.set_value(text, window, cx));
        // Setting the field emits no change: without this the plan shown,
        // and the one confirmed, would be the last pattern's.
        self.preview(cx);
    }

    /// The typed pattern differs from the named one chosen, or none is.
    fn modified(&self, cx: &App) -> bool {
        let typed = self.pattern(cx);
        !typed.is_empty()
            && self
                .chosen
                .and_then(|i| self.patterns.get(i))
                .is_none_or(|p| p.pattern.trim() != typed)
    }

    /// What saving under the name typed would do, said before it is done.
    fn save_preview(&self, cx: &App) -> Option<String> {
        let name = self.save_name.read(cx).value().trim().to_owned();
        if name.is_empty() {
            return None;
        }
        Some(if self.patterns.iter().any(|p| p.name == name) {
            format!("Replaces the pattern “{name}” in config.toml.")
        } else {
            format!("Stored in config.toml as “{name}”, for every kōan on this config.")
        })
    }

    /// Stores the typed pattern under the name typed, then chooses it.
    fn save_pattern(&mut self, cx: &mut Context<Self>) {
        let name = self.save_name.read(cx).value().trim().to_owned();
        let pattern = self.pattern(cx);
        if name.is_empty() || pattern.is_empty() {
            return;
        }
        let engine = engine(cx);
        cx.spawn(async move |this, cx| {
            let saved = engine
                .clone()
                .save_organize_pattern(name.clone(), pattern)
                .await;
            let patterns = engine.organize_patterns().await;
            this.update(cx, |this, cx| {
                match saved {
                    Ok(()) => {
                        this.chosen = patterns.iter().position(|p| p.name == name);
                        this.patterns = patterns;
                        this.saved = Some(format!("Saved as “{name}”."));
                    }
                    Err(e) => this.saved = Some(crate::settings::model::describe(&e)),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Asks with the plan's counts, then moves the files.
    fn ask_to_run(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(plan), Some(base)) = (self.plan.clone(), self.base()) else {
            return;
        };
        if !self.checked || plan.moved_count == 0 {
            return;
        }
        let mut lines: Vec<SharedString> =
            vec![format!("{} move under {base}.", files(plan.moved_count as u64)).into()];
        if plan.unchanged_count > 0 {
            lines.push(
                format!(
                    "{} already where the pattern puts them.",
                    files(plan.unchanged_count as u64)
                )
                .into(),
            );
        }
        if plan.conflict_count + plan.error_count > 0 {
            lines.push(
                format!(
                    "{} stay where they are: their destination is taken or the pattern fails for them.",
                    files((plan.conflict_count + plan.error_count) as u64)
                )
                .into(),
            );
        }
        let Some(selection) = self.selection.clone() else {
            return;
        };
        let (pattern, ancillary) = (self.pattern(cx), self.move_ancillary);
        let sheet = cx.entity().downgrade();
        ask::confirm(
            ask::Confirm {
                title: "organise files".into(),
                verb: "move files".into(),
                destructive: true,
                lines,
                note: Some(if self.move_ancillary {
                    "Cover art, cue sheets and logs move with them. Nothing is overwritten.".into()
                } else {
                    "Nothing is overwritten.".into()
                }),
            },
            window,
            cx,
            move |cx| {
                let Some(sheet) = sheet.upgrade() else { return };
                // The plan previewed, from the selection it was previewed
                // against and with the ancillary choice shown: refused, with
                // nothing moved, if the library has changed since.
                let run = engine(cx).organize_execute_previewed(
                    selection.clone(),
                    pattern.clone(),
                    base.clone(),
                    ancillary,
                );
                // Escape leaves the sheet while files move, as its cancel
                // is disabled: dropping it would stop the move part way.
                ask::hold(true, cx);
                sheet.update(cx, |this, cx| {
                    this.running = true;
                    // A preview still in flight is for the plan before the
                    // move; it must not land over the result.
                    this.generation += 1;
                    this._preview = None;
                    this._settle = None;
                    cx.notify();
                    this._run = Some(cx.spawn(async move |this, cx| {
                        let result = run.await;
                        this.update(cx, |this, cx| {
                            this.running = false;
                            ask::hold(false, cx);
                            match result {
                                // Every file went: done.
                                Ok(done) if done.conflict_count + done.error_count == 0 => {
                                    let moved = done.moved_count as u64;
                                    crate::notice::show(
                                        format!("Moved {}.", files(moved)),
                                        Tone::Muted,
                                        cx,
                                    );
                                    ask::close(cx);
                                }
                                // Some stayed: the sheet shows which, and why.
                                Ok(done) => {
                                    this.error = Some(format!(
                                        "Moved {}; {} stayed where they were, listed below.",
                                        files(done.moved_count as u64),
                                        files((done.conflict_count + done.error_count) as u64)
                                    ));
                                    this.plan = Some(done);
                                    this.checked = false;
                                    this.reread(cx);
                                }
                                // Read again, so the plan shown is the one a
                                // second confirm would run.
                                Err(e) => {
                                    this.error = Some(crate::settings::model::describe(&e));
                                    this.plan = None;
                                    this.reread(cx);
                                }
                            }
                            cx.notify();
                        })
                        .ok();
                    }));
                });
            },
        );
    }

    fn summary(&self) -> Option<(String, bool)> {
        if let Some(error) = &self.error {
            return Some((error.clone(), true));
        }
        if self.folders.is_empty() {
            return Some((
                "There is no library folder to move files into. Add one in Settings → Library."
                    .into(),
                true,
            ));
        }
        let plan = self.plan.as_ref()?;
        let mut parts = vec![format!("{} to move", plan.moved_count)];
        if plan.unchanged_count > 0 {
            parts.push(format!("{} unchanged", plan.unchanged_count));
        }
        if plan.conflict_count > 0 {
            parts.push(format!("{} blocked", plan.conflict_count));
        }
        if plan.error_count > 0 {
            parts.push(format!("{} failed", plan.error_count));
        }
        if plan.unresolved > 0 {
            parts.push(format!("{} not on this device", plan.unresolved));
        }
        if !self.checked {
            parts.push("checking the disk…".into());
        }
        Some((
            parts.join(" · "),
            plan.conflict_count + plan.error_count > 0,
        ))
    }
}

/// "1 file", "12 files".
fn files(n: u64) -> String {
    crate::library::chrome::count(n, "file", "files")
}

/// What one file's row says beside its destination.
fn outcome(entry: &OrganizeEntry) -> (String, bool) {
    match entry.outcome {
        PlanOutcome::Move if entry.ancillary.is_empty() => ("moves".into(), false),
        PlanOutcome::Move => (format!("moves, with {}", entry.ancillary.join(", ")), false),
        PlanOutcome::Unchanged => ("already there".into(), false),
        _ => (entry.reason.clone().unwrap_or_else(|| "stays".into()), true),
    }
}

impl Render for OrganizeSheet {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Paint::of(window, cx);
        let look = &p.look;
        let ready = self.checked
            && !self.running
            && self.plan.as_ref().is_some_and(|plan| plan.moved_count > 0);
        let running = self.running;
        let cancel = Button::new("organize-cancel", ButtonKind::Standard)
            .label("cancel")
            .disabled(running)
            .on_click(|_, _, cx| ask::close(cx));
        let go = Button::new("organize-go", ButtonKind::Standard)
            .label(if self.running {
                "moving…"
            } else {
                "organise"
            })
            .disabled(!ready)
            .on_click(cx.listener(|this, _, window, cx| this.ask_to_run(window, cx)));

        let mut names: Vec<String> = self.patterns.iter().map(|p| p.name.clone()).collect();
        names.push("typed here".into());
        // A named pattern edited is no longer that pattern: the picker says
        // it is typed here, and names it again if the text comes back.
        let named = self.chosen.filter(|_| !self.modified(cx));
        let chosen = Some(named.unwrap_or(names.len() - 1));
        let picker = Picker::keeps_case("organize-pattern", names, chosen)
            .disabled(running)
            .on_change(cx.listener(|this, index: &usize, window, cx| {
                this.choose_pattern(*index, window, cx)
            }));
        let folder = Picker::keeps_case("organize-folder", self.folders.clone(), Some(self.folder))
            .disabled(running)
            .on_change(cx.listener(|this, index: &usize, _, cx| {
                this.folder = *index;
                this.preview(cx);
            }));
        let ancillary = Toggle::new(
            "organize-ancillary",
            "move cover art, cue sheets and logs with the music",
            self.move_ancillary,
        )
        .disabled(running)
        .on_change(cx.listener(|this, on: &bool, _, cx| {
            this.move_ancillary = *on;
            let enabled = *on;
            let set = engine(cx).set_organize_moves_ancillary(enabled);
            cx.background_executor().spawn(set).detach();
            this.preview(cx);
        }));

        let entries = self.plan.as_ref().map_or(0, |plan| plan.entries.len());
        let list = table_list(
            "organize-plan",
            entries,
            cx.processor(move |this, range: Range<usize>, window, cx| {
                let p = Paint::of(window, cx);
                let Some(plan) = &this.plan else {
                    return Vec::new();
                };
                range
                    .map(|i| {
                        let entry = &plan.entries[i];
                        let (said, warns) = outcome(entry);
                        TableRow::new(("organize-row", i), RowKind::Text)
                            .title(
                                div()
                                    .text_role(TypeRole::Body, &p.look)
                                    .text_color(p.ink)
                                    .truncate()
                                    .child(entry.to_path.clone().unwrap_or_default()),
                            )
                            .line(
                                div()
                                    .text_role(TypeRole::Meta, &p.look)
                                    .text_color(p.muted)
                                    .truncate()
                                    .child(entry.from_path.clone()),
                            )
                            .figure(
                                OUTCOME,
                                div()
                                    .text_role(TypeRole::Fine, &p.look)
                                    .text_color(if warns { p.bad } else { p.muted })
                                    .truncate()
                                    .child(said),
                            )
                    })
                    .collect()
            }),
        )
        .flex_1()
        .min_h_0();

        ask::wide_panel(crate::theme::label("organise"), look)
            .child(ask::head_data(
                format!("{} {}", crate::theme::label("organise"), self.title),
                vec![cancel.into_any_element(), go.into_any_element()],
                look,
            ))
            .child(
                ask::body("organize-form", look)
                    .flex_none()
                    .pt(look.space(crate::theme::Space::M))
                    .child(FormRow::new("pattern").control(picker))
                    .child(
                        TextField::new(&self.draft)
                            .label(crate::theme::label("pattern"))
                            .disabled(running),
                    )
                    .when(self.modified(cx), |d| {
                        let preview = self.save_preview(cx);
                        d.child(
                            FormRow::new("save as").control(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(look.space(crate::theme::Space::S))
                                    .child(
                                        TextField::new(&self.save_name)
                                            .label(crate::theme::label("save as"))
                                            .disabled(running),
                                    )
                                    .child(
                                        Button::new("organize-save", ButtonKind::Bordered)
                                            .label("save")
                                            .disabled(running || preview.is_none())
                                            .on_click(
                                                cx.listener(|this, _, _, cx| this.save_pattern(cx)),
                                            ),
                                    ),
                            ),
                        )
                        .children(preview.map(|text| ask::note(text, Tone::Muted, look)))
                    })
                    .children(
                        self.saved
                            .clone()
                            .map(|text| ask::note(text, Tone::Muted, look)),
                    )
                    .child(FormRow::new("library folder").control(folder))
                    .child(ancillary)
                    .children(self.summary().map(|(text, warns)| {
                        ask::note(text, if warns { Tone::Bad } else { Tone::Muted }, look)
                    })),
            )
            .child(list)
    }
}

#[cfg(test)]
mod pattern_tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn a_pattern_s_syntax_is_coloured() {
        let pattern = "%album artist%/[%date% - ]$if2(%album%,'none')/é";
        let spans: Vec<(&str, Tone)> = pattern_spans(pattern)
            .into_iter()
            .map(|(r, t)| (&pattern[r], t))
            .collect();
        assert_eq!(
            spans,
            [
                ("%album artist%", Tone::Accent),
                ("[", Tone::Muted),
                ("%date%", Tone::Accent),
                ("]", Tone::Muted),
                ("$if2(", Tone::Strong),
                ("%album%", Tone::Accent),
                (",", Tone::Muted),
                ("'none'", Tone::Muted),
                (")", Tone::Muted),
            ]
        );
    }

    #[test]
    fn an_unclosed_field_runs_to_the_end() {
        assert_eq!(pattern_spans("a/%alb"), [(2..6, Tone::Accent)]);
    }
}
