//! Every correction, EQ and preset, where each is used, and what can be done
//! to it: open, rename, copy, revert, keep on every device, delete. Groups
//! are headings with their members under them. The EQ page shows one device;
//! this shows them all.

use std::collections::HashSet;
use std::rc::Rc;

use koan_kit::app::input::{InputEvent, InputState};
use koan_kit::app::*;
use koan_runtime::{DeletedDsp, DspOverview, DspProfileSummary, DspRole, DspTuningEntry};

use super::flows;
use super::menu::{Item, Trigger, menu};
use super::model::{DELETED_EXTENSION, Dsp, SYNC_EXTENSION, describe, describe_profile, joined};
use super::page::EqPage;
use super::profile::ProfilePage;
use super::sheet::{Confirm, Sheets, body, head, note, section};
use crate::components::{Button, ButtonKind, SectionHeader, TextField};
use crate::theme::{KoanStyled, Look, Space, Tone, TypeRole, label};

pub struct ManageEq {
    dsp: Entity<Dsp>,
    sheets: Entity<Sheets>,
    /// The device the EQ page shows, which Add to Filters adds to.
    device: Option<String>,
    chain: Vec<DspTuningEntry>,
    active: Option<String>,
    /// A rename or a copy being named: which, of what, and the field.
    naming: Option<(Naming, String, Entity<InputState>, Subscription)>,
    /// EQs the server keeps after deletion: none until the first read
    /// answers, and the last list kept while another is read.
    deleted: Option<Vec<DeletedDsp>>,
    restoring: Option<String>,
    restore_error: Option<SharedString>,
    loaded: Option<((u64, u64), bool)>,
    /// The read under way; a newer one drops it.
    _load: Option<koan_kit::app::Task<()>>,
    _subscriptions: Vec<Subscription>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Naming {
    Rename,
    Copy,
}

impl ManageEq {
    pub fn new(
        dsp: Entity<Dsp>,
        sheets: Entity<Sheets>,
        device: Option<String>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscriptions = vec![cx.observe(&dsp, |m: &mut Self, _, cx| m.load(cx))];
        let mut manage = Self {
            dsp,
            sheets,
            device,
            chain: Vec::new(),
            active: None,
            naming: None,
            deleted: None,
            restoring: None,
            restore_error: None,
            loaded: None,
            _load: None,
            _subscriptions: subscriptions,
        };
        manage.load(cx);
        manage
    }

    /// Recently deleted is the server's, so it is shown only while one that
    /// keeps them is in reach.
    fn offered(&self, cx: &App) -> bool {
        let d = self.dsp.read(cx);
        d.offers(DELETED_EXTENSION) && !d.offline
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let key = (self.dsp.read(cx).stamp(), self.offered(cx));
        if self.loaded == Some(key) {
            cx.notify();
            return;
        }
        self.loaded = Some(key);
        let engine = self.dsp.read(cx).engine();
        let (device, offered) = (self.device.clone(), key.1);
        self._load = Some(cx.spawn(async move |this, cx| {
            let chain = match &device {
                Some(d) => Some(engine.clone().dsp_overview_for(Some(d.clone())).await),
                None => None,
            };
            // A failed read is not an empty one: the last list stays.
            let deleted = if offered {
                engine.dsp_deleted().await.ok()
            } else {
                None
            };
            this.update(cx, |m, cx| {
                if let Some(o) = chain {
                    m.chain = o.chain;
                    m.active = o.active;
                }
                if !offered {
                    m.deleted = None;
                } else if deleted.is_some() {
                    m.deleted = deleted;
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn open(&self, name: String, window: &mut Window, cx: &mut App) {
        let (dsp, sheets, device) = (self.dsp.clone(), self.sheets.clone(), self.device.clone());
        let view = cx.new(|cx| ProfilePage::new(dsp, sheets.clone(), name, device, window, cx));
        sheets.update(cx, |s, cx| s.view(view, cx));
    }

    fn name(&mut self, naming: Naming, of: String, window: &mut Window, cx: &mut Context<Self>) {
        let start = match naming {
            Naming::Rename => of.clone(),
            Naming::Copy => format!("{of} copy"),
        };
        let input = cx.new(|cx| InputState::new(window, cx).default_value(start));
        let sub = cx.subscribe_in(
            &input,
            window,
            |m: &mut Self, _, e: &InputEvent, _, cx| match e {
                InputEvent::PressEnter { .. } => m.finish_naming(cx),
                InputEvent::Change => cx.notify(),
                _ => {}
            },
        );
        self.naming = Some((naming, of, input, sub));
        cx.notify();
    }

    fn finish_naming(&mut self, cx: &mut Context<Self>) {
        let Some((naming, of, input, _)) = self.naming.take() else {
            return;
        };
        let new = input.read(cx).value().trim().to_owned();
        cx.notify();
        if naming == Naming::Rename && (new.is_empty() || new == of) {
            return;
        }
        // Renaming changes what every device and preset holding it names;
        // say so first.
        match naming {
            Naming::Rename => {
                let held = self.dsp.read(cx).profile(&of).cloned();
                let dsp = self.dsp.clone();
                let mut lines = vec![format!("{of} is renamed {new} everywhere it is used.")];
                if let Some(p) = held {
                    lines.extend(used(&p, &self.dsp.read(cx).overview));
                }
                let (from, to) = (of.clone(), new.clone());
                self.sheets.update(cx, |s, cx| {
                    s.confirm(
                        Confirm {
                            title: format!("Rename {of}?").into(),
                            message: lines.join(" ").into(),
                            action: "Rename".into(),
                            destructive: false,
                            also: None,
                            run: Rc::new(move |_, cx| {
                                let (from, to) = (from.clone(), to.clone());
                                dsp.update(cx, |d, cx| d.act(cx, move |e| e.dsp_rename(from, to)));
                            }),
                        },
                        cx,
                    )
                });
            }
            Naming::Copy => {
                // Unnamed, the copy is "<name> copy".
                let new = (!new.is_empty()).then_some(new);
                self.dsp.update(cx, |d, cx| {
                    d.act(cx, move |e| {
                        let copied = e.dsp_duplicate(of, new);
                        async move { copied.await.map(|_| ()) }
                    })
                });
            }
        }
    }

    /// Adds `names` to the end of the device's filters, as they are when
    /// the write runs rather than as this page last read them.
    fn add_to_filters(&self, names: &[String], cx: &mut App) {
        let Some(device) = self.device.clone() else {
            return;
        };
        let names = names.to_vec();
        self.dsp.update(cx, |d, cx| {
            d.act(cx, move |e| async move {
                let mut chain = e.clone().dsp_overview_for(Some(device.clone())).await.chain;
                for n in names {
                    if !chain.iter().any(|entry| entry.name == n) {
                        chain.push(DspTuningEntry { name: n, on: true });
                    }
                }
                e.dsp_set_tunings(device, chain).await
            })
        });
    }

    fn actions(&self, p: &DspProfileSummary, cx: &mut Context<Self>) -> Vec<Item> {
        let this = cx.weak_entity();
        let dsp = self.dsp.read(cx);
        let mut items = vec![{
            let (this, name) = (this.clone(), p.name.clone());
            Item::action(label("Open"), move |window, cx| {
                this.update(cx, |m, cx| m.open(name.clone(), window, cx))
                    .ok();
            })
        }];
        if let Some(device) = self.device.clone() {
            let shown = dsp.label(&device);
            if p.preset {
                let (dsp, sheets, name, device) = (
                    self.dsp.clone(),
                    self.sheets.clone(),
                    p.name.clone(),
                    device.clone(),
                );
                items.push(Item::action(
                    format!("{} {shown}", label("Use on")),
                    move |_, cx| {
                        super::page::set_from_preset(
                            &dsp,
                            &sheets,
                            &device,
                            Some(name.clone()),
                            cx,
                        );
                    },
                ));
            } else if p.role == DspRole::Tuning
                && p.rates.is_empty()
                && !self.chain.iter().any(|e| e.name == p.name)
            {
                let (this, name) = (this.clone(), p.name.clone());
                items.push(Item::action(label("Add to Filters"), move |_, cx| {
                    this.update(cx, |m, cx| {
                        m.add_to_filters(std::slice::from_ref(&name), cx)
                    })
                    .ok();
                }));
            } else if p.role != DspRole::Tuning && self.active.as_ref() != Some(&p.name) {
                let (page_dsp, name) = (self.dsp.clone(), p.name.clone());
                items.push(Item::action(label("Use as Correction"), move |_, cx| {
                    page_dsp.update(cx, |d, cx| d.assign(Some(name.clone()), device.clone(), cx));
                }));
            }
        }
        items.push(Item::Rule);
        for (naming, word) in [(Naming::Rename, "Rename…"), (Naming::Copy, "Save as New…")] {
            let (this, name) = (this.clone(), p.name.clone());
            items.push(Item::action(label(word), move |window, cx| {
                this.update(cx, |m, cx| m.name(naming, name.clone(), window, cx))
                    .ok();
            }));
        }
        if p.edited {
            let (sheets, dsp, name) = (self.sheets.clone(), self.dsp.clone(), p.name.clone());
            items.push(Item::action(label("Revert to Imported"), move |_, cx| {
                let (dsp, n) = (dsp.clone(), name.clone());
                sheets.update(cx, |s, cx| {
                    s.confirm(
                        Confirm {
                            title: format!("Revert {name} to its file?").into(),
                            message: "Every filter and the headroom go back to how the file had them; the edits made since are lost.".into(),
                            action: "Revert".into(),
                            destructive: true,
                            also: None,
                            run: Rc::new(move |_, cx| dsp.update(cx, |d, cx| d.revert(n.clone(), cx))),
                        },
                        cx,
                    )
                });
            }));
        }
        if dsp.offers(SYNC_EXTENSION) {
            match &p.scope_locked {
                Some(why) => items.push(Item::Heading(
                    format!(
                        "{}: {why}",
                        if p.everywhere {
                            "Kept on every device"
                        } else {
                            "Kept on this device"
                        }
                    )
                    .into(),
                )),
                None => {
                    let (sheets, dsp, name, everywhere) = (
                        self.sheets.clone(),
                        self.dsp.clone(),
                        p.name.clone(),
                        !p.everywhere,
                    );
                    let word = if everywhere {
                        "Keep on Every Device"
                    } else {
                        "Keep on This Device Only"
                    };
                    items.push(Item::action(label(word), move |_, cx| {
                        let (dsp, n) = (dsp.clone(), name.clone());
                        let message = if everywhere {
                            format!("{name} is sent to every device signed in to your kōan server, and an edit on one reaches the rest.")
                        } else {
                            format!("{name} stays on this device and is removed from your other devices.")
                        };
                        sheets.update(cx, |s, cx| {
                            s.confirm(
                                Confirm {
                                    title: label(word),
                                    message: message.into(),
                                    action: if everywhere { "Keep Everywhere".into() } else { "Keep Here".into() },
                                    destructive: !everywhere,
                                    also: None,
                                    run: Rc::new(move |_, cx| {
                                        let n = n.clone();
                                        dsp.update(cx, |d, cx| d.act(cx, move |e| e.dsp_set_scope(n, everywhere)));
                                    }),
                                },
                                cx,
                            )
                        });
                    }));
                }
            }
        }
        items.push(Item::Rule);
        let (sheets, page_dsp, name, lines) = (
            self.sheets.clone(),
            self.dsp.clone(),
            p.name.clone(),
            used(p, &dsp.overview),
        );
        items.push(Item::destructive(label("Delete…"), move |_, cx| {
            let (dsp, n) = (page_dsp.clone(), name.clone());
            let message = if lines.is_empty() {
                "It is not used anywhere.".to_owned()
            } else {
                lines.join(" ")
            };
            sheets.update(cx, |s, cx| {
                s.confirm(
                    Confirm {
                        title: format!("Delete {name}?").into(),
                        message: message.into(),
                        action: "Delete".into(),
                        destructive: true,
                        also: None,
                        run: Rc::new(move |_, cx| dsp.update(cx, |d, cx| d.delete(n.clone(), cx))),
                    },
                    cx,
                )
            });
        }));
        items
    }

    fn row(
        &self,
        p: &DspProfileSummary,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let look = Look::of(window, cx);
        let dsp = self.dsp.read(cx);
        let detail = match &p.problem {
            Some(problem) => note(problem.clone(), Tone::Bad, &look),
            None => {
                let used = if p.used_on.is_empty() {
                    "not used".to_owned()
                } else {
                    // A part of a line that starts with the bands, so in
                    // the line's case.
                    format!(
                        "used on {}",
                        joined(&p.used_on.iter().map(|d| dsp.label(d)).collect::<Vec<_>>())
                    )
                };
                let what = describe_profile(p);
                note(
                    [what, used]
                        .into_iter()
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join(" · "),
                    Tone::Muted,
                    &look,
                )
            }
        };
        let items = self.actions(p, cx);
        let (this, name) = (cx.weak_entity(), p.name.clone());
        div()
            .flex()
            .items_center()
            .gap(look.space(Space::M))
            .py(look.space(Space::S))
            .child(
                div()
                    .id(SharedString::from(format!("manage-{}", p.name)))
                    .flex_1()
                    .min_w_0()
                    .cursor_pointer()
                    .on_click(move |_, window, cx| {
                        this.update(cx, |m, cx| m.open(name.clone(), window, cx))
                            .ok();
                    })
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(look.space(Space::S))
                            .child(
                                div()
                                    .text_role(TypeRole::Body, &look)
                                    .text_color(look.palette.ink.hsla())
                                    .child(p.name.clone()),
                            )
                            .when(p.edited, |d| {
                                d.child(note(label("Edited"), Tone::Muted, &look))
                            }),
                    )
                    .child(detail),
            )
            .child(menu(
                SharedString::from(format!("manage-options-{}", p.name)),
                label("Options"),
                Trigger::Word,
                items,
                window,
                cx,
            ))
            .into_any_element()
    }

    fn list(
        &self,
        title: &str,
        profiles: Vec<&DspProfileSummary>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if profiles.is_empty() {
            return None;
        }
        let look = Look::of(window, cx);
        let rows = profiles
            .into_iter()
            .map(|p| self.row(p, window, cx))
            .collect();
        Some(section(Some(title), rows, None, &look).into_any_element())
    }

    fn group(
        &self,
        group: &DspProfileSummary,
        o: &DspOverview,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let look = Look::of(window, cx);
        let members: Vec<&DspProfileSummary> = group
            .members
            .iter()
            .filter_map(|n| o.profiles.iter().find(|p| &p.name == n))
            .collect();
        let rows: Vec<AnyElement> = members
            .into_iter()
            .map(|p| self.row(p, window, cx))
            .collect();
        let items = self.actions(group, cx);
        let add_all = (self.device.is_some() && group.role == DspRole::Tuning).then(|| {
            let (this, names) = (cx.weak_entity(), group.members.clone());
            Button::new(
                SharedString::from(format!("add-all-{}", group.name)),
                ButtonKind::Link,
            )
            .label("Add All to Filters")
            .on_click(move |_, _, cx| {
                this.update(cx, |m, cx| m.add_to_filters(&names, cx)).ok();
            })
        });
        div()
            .flex()
            .flex_col()
            .gap(look.space(Space::Xs))
            .child(
                div()
                    .flex()
                    .items_end()
                    .justify_between()
                    .gap(look.space(Space::M))
                    .child(
                        div()
                            .pt(look.space(Space::L))
                            .text_role(TypeRole::Fine, &look)
                            .text_color(look.palette.ink.hsla())
                            .child(group.name.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(look.space(Space::M))
                            .children(add_all)
                            .child(menu(
                                SharedString::from(format!("group-options-{}", group.name)),
                                label("Options"),
                                Trigger::Word,
                                items,
                                window,
                                cx,
                            )),
                    ),
            )
            .children(rows)
            .into_any_element()
    }

    fn restore(&mut self, uid: String, cx: &mut Context<Self>) {
        self.restoring = Some(uid.clone());
        cx.notify();
        let engine = self.dsp.read(cx).engine();
        let dsp = self.dsp.clone();
        cx.spawn(async move |this, cx| {
            let result = engine.dsp_restore(uid).await;
            this.update(cx, |m, cx| {
                m.restoring = None;
                m.restore_error = result.err().map(|e| describe(&e));
                m.loaded = None;
                cx.notify();
            })
            .ok();
            dsp.update(cx, |d, cx| d.reload(cx));
        })
        .detach();
    }

    fn recently_deleted(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let deleted = self.deleted.clone()?;
        let look = Look::of(window, cx);
        let mut rows: Vec<AnyElement> = Vec::new();
        if deleted.is_empty() {
            rows.push(
                note(
                    label("Nothing deleted in the last thirty days"),
                    Tone::Muted,
                    &look,
                )
                .into_any_element(),
            );
        }
        for d in deleted {
            let (this, uid) = (cx.weak_entity(), d.uid.clone());
            let days = if d.days_left == 1 {
                "1 day left".to_owned()
            } else {
                format!("{} days left", d.days_left)
            };
            rows.push(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .py(look.space(Space::S))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_role(TypeRole::Body, &look)
                                    .text_color(look.palette.ink.hsla())
                                    .child(d.name.clone()),
                            )
                            .child(note(days, Tone::Muted, &look)),
                    )
                    .child(
                        Button::new(
                            SharedString::from(format!("restore-{}", d.uid)),
                            ButtonKind::Bordered,
                        )
                        .label("Restore")
                        .disabled(self.restoring.is_some())
                        .on_click(move |_, _, cx| {
                            this.update(cx, |m, cx| m.restore(uid.clone(), cx)).ok();
                        }),
                    )
                    .into_any_element(),
            );
        }
        let footer: SharedString = self.restore_error.clone().unwrap_or_else(|| {
            "Kept on the server for thirty days after they were deleted. Restoring one brings it back on every device.".into()
        });
        Some(section(Some("Recently deleted"), rows, Some(footer), &look).into_any_element())
    }
}

/// Where a profile goes from when deleted or renamed: the devices that play
/// it and the presets and groups that hold it.
fn used(p: &DspProfileSummary, o: &Option<DspOverview>) -> Vec<String> {
    let label_of = |d: &String| {
        o.as_ref()
            .and_then(|o| o.names.get(d).cloned())
            .unwrap_or_else(|| d.clone())
    };
    let mut lines = Vec::new();
    if !p.used_on.is_empty() {
        lines.push(format!(
            "Used on {}, which will play without it.",
            joined(&p.used_on.iter().map(label_of).collect::<Vec<_>>())
        ));
    }
    if !p.held_by.is_empty() {
        lines.push(format!("It is taken out of {}.", joined(&p.held_by)));
    }
    lines
}

impl Render for ManageEq {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let sheets = self.sheets.clone();
        let done = Button::new("manage-done", ButtonKind::Standard)
            .label("Done")
            .on_click(move |_, _, cx| sheets.update(cx, |s, cx| s.pop(cx)));
        let o = self.dsp.read(cx).overview.clone();
        let mut sections: Vec<AnyElement> = Vec::new();
        if let Some((naming, of, input)) = self
            .naming
            .as_ref()
            .map(|(n, o, i, _)| (*n, o.clone(), i.clone()))
        {
            let this = cx.weak_entity();
            let cancel = this.clone();
            sections.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(look.space(Space::S))
                    .child(SectionHeader::new(match naming {
                        Naming::Rename => "Rename",
                        Naming::Copy => "Save as New",
                    }))
                    .when(naming == Naming::Copy, |d| {
                        d.child(note(
                            format!(
                                "A copy of {of} as it is now, to change apart from the original."
                            ),
                            Tone::Muted,
                            &look,
                        ))
                    })
                    .child(
                        div()
                            .flex()
                            .gap(look.space(Space::M))
                            .child(div().flex_1().child(TextField::new(&input)))
                            .child(
                                Button::new("naming-go", ButtonKind::Bordered)
                                    .label("Save")
                                    .on_click(move |_, _, cx| {
                                        this.update(cx, |m, cx| m.finish_naming(cx)).ok();
                                    }),
                            )
                            .child(
                                Button::new("naming-cancel", ButtonKind::Link)
                                    .label("Cancel")
                                    .on_click(move |_, _, cx| {
                                        cancel
                                            .update(cx, |m, cx| {
                                                m.naming = None;
                                                cx.notify();
                                            })
                                            .ok();
                                    }),
                            ),
                    )
                    .into_any_element(),
            );
        }
        if let Some(o) = &o {
            let grouped: HashSet<&String> =
                o.profiles.iter().flat_map(|p| p.members.iter()).collect();
            let loose: Vec<&DspProfileSummary> = o
                .profiles
                .iter()
                .filter(|p| !p.preset && p.members.is_empty() && !grouped.contains(&p.name))
                .collect();
            let corrections = loose
                .iter()
                .copied()
                .filter(|p| p.role != DspRole::Tuning)
                .collect();
            let eqs = loose
                .iter()
                .copied()
                .filter(|p| p.role == DspRole::Tuning)
                .collect();
            sections.extend(self.list("Corrections", corrections, window, cx));
            sections.extend(self.list("EQs", eqs, window, cx));
            for g in o.profiles.iter().filter(|p| !p.members.is_empty()) {
                sections.push(self.group(g, o, window, cx));
            }
            sections.extend(self.list(
                "Presets",
                o.profiles.iter().filter(|p| p.preset).collect(),
                window,
                cx,
            ));
        }
        sections.extend(self.recently_deleted(window, cx));
        let (dsp, sheets) = (self.dsp.clone(), self.sheets.clone());
        let adds = div()
            .flex()
            .flex_wrap()
            .gap(look.space(Space::M))
            .child(
                Button::new("manage-import", ButtonKind::Bordered)
                    .label("Import a File…")
                    .on_click({
                        let dsp = dsp.clone();
                        move |_, _, cx| EqPage::import(dsp.clone(), None, cx)
                    }),
            )
            .child(
                Button::new("manage-autoeq", ButtonKind::Bordered)
                    .label("Find in AutoEQ…")
                    .on_click({
                        let (dsp, sheets) = (dsp.clone(), sheets.clone());
                        move |_, window, cx| {
                            let view = cx.new(|cx| {
                                flows::AutoEqSearch::new(
                                    dsp.clone(),
                                    sheets.clone(),
                                    String::new(),
                                    window,
                                    cx,
                                )
                            });
                            sheets.update(cx, |s, cx| s.view(view, cx));
                        }
                    }),
            )
            .child(
                Button::new("manage-measure", ButtonKind::Bordered)
                    .label("Find a Measurement…")
                    .on_click({
                        let (dsp, sheets) = (dsp.clone(), sheets.clone());
                        move |_, window, cx| {
                            let view = cx.new(|cx| {
                                flows::MeasurementFlow::new(
                                    dsp.clone(),
                                    sheets.clone(),
                                    String::new(),
                                    window,
                                    cx,
                                )
                            });
                            sheets.update(cx, |s, cx| s.view(view, cx));
                        }
                    }),
            );
        let d = self.dsp.read(cx);
        let mut add_rows = vec![adds.into_any_element()];
        if let Some(s) = &d.import_summary {
            add_rows.push(note(s.clone(), Tone::Muted, &look).into_any_element());
        }
        if let Some(e) = &d.last_error {
            add_rows.push(note(e.clone(), Tone::Bad, &look).into_any_element());
        }
        sections.push(
            section(
                Some("Add"),
                add_rows,
                Some("AutoEQ and Equalizer APO text, impulse WAVs, Roon zips, Convolver .cfg and CamillaDSP configs, or a headphone found in AutoEQ by name. Importing under a name already used adds to that EQ; several files at once become a group. Files dropped on the EQ page import too.".into()),
                &look,
            )
            .into_any_element(),
        );
        div()
            .flex()
            .flex_col()
            .child(head("Manage EQ", vec![done.into_any_element()], &look))
            .child(body("manage-body", &look).children(sections))
    }
}
