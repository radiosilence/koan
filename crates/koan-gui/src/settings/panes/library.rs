//! Library: the folders scanned, scanning them, and clearing the index.

use crate::components::{Button, ButtonKind};
use crate::theme::{Icon, KoanStyled, Look, Space, Tone, TypeRole};
use koan_kit::raw::prelude::*;
use koan_kit::raw::*;

use crate::settings::Ctx;
use crate::settings::form::{buttons, page, section, text};
use crate::settings::model::{Uses, count};
use crate::settings::sheet::{Action, Sheet};

pub struct LibraryPane {
    ctx: Ctx,
    choosing: Option<Task<()>>,
    _model: Subscription,
}

impl LibraryPane {
    pub fn new(ctx: Ctx, cx: &mut Context<Self>) -> Self {
        Self {
            _model: cx.observe(&ctx.model, |_, _, cx| cx.notify()),
            ctx,
            choosing: None,
        }
    }

    fn add_folder(&mut self, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: true,
            prompt: Some("Add".into()),
        });
        let model = self.ctx.model.clone();
        self.choosing = Some(cx.spawn(async move |_, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else {
                return;
            };
            let paths = paths
                .into_iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect();
            model.update(cx, |model, cx| model.add_folders(paths, cx));
        }));
    }

    /// Asks before a folder goes: kept rows stay in the library, forgotten
    /// ones go, and its files are not touched either way.
    fn remove(&self, path: String, tracks: u64, window: &mut Window, cx: &mut App) {
        let keep = {
            let model = self.ctx.model.clone();
            let path = path.clone();
            move |_: &[String], _: &mut Window, cx: &mut App| {
                model.update(cx, |model, cx| model.remove_folder(path.clone(), false, cx))
            }
        };
        let forget = {
            let model = self.ctx.model.clone();
            let path = path.clone();
            move |_: &[String], _: &mut Window, cx: &mut App| {
                model.update(cx, |model, cx| model.remove_folder(path.clone(), true, cx))
            }
        };
        let sheet = Sheet::new(format!("Stop scanning {path}?"))
            .message(format!(
                "Its {} tracks are in the library now. Keeping them leaves records kōan will not scan again; forgetting them removes them. Your files are not touched either way.",
                count(tracks)
            ))
            .action(Action::new("Remove, Keep Them in the Library", keep))
            .action(Action::new("Remove and Forget Its Tracks", forget).destructive());
        self.ctx
            .sheets
            .update(cx, |sheets, cx| sheets.show(sheet, window, cx));
    }

    fn rebuild(&self, window: &mut Window, cx: &mut App) {
        let model = self.ctx.model.clone();
        let sheet = Sheet::new("Clear the library index?")
            .message("Every artist, album and track is forgotten, so the next scan or sync builds them again. Play counts, lyrics and audio analysis are lost with them. Favourites are kept. Your music files are not touched.")
            .action(
                Action::new("Clear Index", move |_, _, cx| {
                    model.update(cx, |model, cx| model.rebuild_index(cx))
                })
                .destructive(),
            );
        self.ctx
            .sheets
            .update(cx, |sheets, cx| sheets.show(sheet, window, cx));
    }
}

impl Render for LibraryPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let model = self.ctx.model.read(cx);
        let Some(settings) = model.settings() else {
            return page("library", &look, Vec::new());
        };
        let scanning = model.conflicts(Uses::LOCAL_LIBRARY);
        let whole = model.conflicts(Uses::WHOLE_LIBRARY);
        let entity = cx.entity();

        let mut folders: Vec<AnyElement> = Vec::new();
        if settings.library_folders.is_empty() {
            folders.push(
                text(
                    "No folders yet — kōan has nothing to scan.",
                    TypeRole::Meta,
                    Tone::Muted,
                    &look,
                )
                .into_any_element(),
            );
        }
        for (index, folder) in settings.library_folders.iter().enumerate() {
            let path = folder.path.clone();
            let tracks = folder.tracks;
            let entity = entity.clone();
            folders.push(
                div()
                    .flex()
                    .items_center()
                    .gap(look.space(Space::M))
                    .py(look.space(Space::Xs))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_role(TypeRole::Meta, &look)
                            .text_color(look.palette.ink.hsla())
                            .truncate()
                            .child(folder.path.clone()),
                    )
                    .child(text(
                        format!("{} tracks", count(folder.tracks)),
                        TypeRole::Fine,
                        Tone::Muted,
                        &look,
                    ))
                    .child(
                        Button::new(("remove-folder", index), ButtonKind::Icon)
                            .label("Stop scanning this folder")
                            .icon(Icon::Remove)
                            .on_click(move |_, window, cx| {
                                entity.update(cx, |this, cx| {
                                    this.remove(path.clone(), tracks, window, cx)
                                })
                            }),
                    )
                    .into_any_element(),
            );
        }
        folders.push(buttons(
            vec![
                // Adding a folder starts a scan, so it waits for the one running.
                Button::new("add-folder", ButtonKind::Bordered)
                    .label("Add Folder…")
                    .disabled(scanning)
                    .on_click({
                        let entity = entity.clone();
                        move |_, _, cx| entity.update(cx, |pane, cx| pane.add_folder(cx))
                    })
                    .into_any_element(),
            ],
            &look,
        ));

        let model_scan = self.ctx.model.clone();
        let model_force = self.ctx.model.clone();
        let scan = buttons(
            vec![
                Button::new("scan", ButtonKind::Bordered)
                    .label("Scan")
                    .disabled(scanning)
                    .on_click(move |_, _, cx| model_scan.update(cx, |m, cx| m.scan(false, cx)))
                    .into_any_element(),
                Button::new("rescan", ButtonKind::Bordered)
                    .label("Rescan Everything")
                    .disabled(scanning)
                    .on_click(move |_, _, cx| model_force.update(cx, |m, cx| m.scan(true, cx)))
                    .into_any_element(),
            ],
            &look,
        );

        let rebuild = buttons(
            vec![
                Button::new("rebuild", ButtonKind::Bordered)
                    .label("Clear Library Index…")
                    .destructive(true)
                    .disabled(whole)
                    .on_click(move |_, window, cx| {
                        entity.update(cx, |this, cx| this.rebuild(window, cx))
                    })
                    .into_any_element(),
            ],
            &look,
        );

        page(
            "library",
            &look,
            vec![
                section(
                    Some("Folders"),
                    folders,
                    Some("Removing a folder stops it being scanned. It does not delete anything.".into()),
                    &look,
                ),
                section(
                    Some("Scan"),
                    vec![scan],
                    // One pass over the files at a time.
                    scanning.then(|| "Waiting for the task that is reading your files to finish.".into()),
                    &look,
                ),
                section(
                    Some("Rebuild"),
                    vec![rebuild],
                    Some("Forgets every artist, album and track so the next scan builds them again from your files. Favourites survive — they are kept against file paths. Lyrics, play counts and audio analysis do not; they are tied to rows that will not exist.".into()),
                    &look,
                ),
            ],
        )
    }
}
