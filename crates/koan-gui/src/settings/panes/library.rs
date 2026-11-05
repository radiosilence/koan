//! Library: the folders scanned, scanning them, and clearing the index.

use crate::components::{Button, ButtonKind};
use crate::theme::{Icon, KoanStyled, Look, Space, Tone, TypeRole, label};
use koan_kit::app::*;

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
        self.choosing = Some(choose_folders(self.ctx.model.clone(), cx));
    }

    fn remove(&self, path: String, tracks: u64, window: &mut Window, cx: &mut App) {
        ask_remove(&self.ctx, path, tracks, window, cx);
    }

    /// Asks before kōan goes back to its defaults, saying what of this
    /// device's setup goes with it.
    fn reset(&self, window: &mut Window, cx: &mut App) {
        let Some(settings) = self.ctx.model.read(cx).settings() else {
            return;
        };
        let mut message = vec![
            "Every setting goes back to how it was when kōan was first opened, and the setup guide opens again.".to_owned(),
        ];
        if settings.share_enabled || self.ctx.sharing.read(cx).serving.is_some() {
            message.push(
                "This device stops sharing its library. The accounts made for it are kept.".into(),
            );
        }
        if settings.remote_signed_in || settings.remote_enabled {
            let host = super::server::host(&settings.remote_url);
            message.push(format!(
                "kōan signs out of {host} and forgets its library here, with the account's favourites, history and playlists. Nothing on the server changes."
            ));
        }
        if !settings.library_folders.is_empty() {
            let tracks: u64 = settings.library_folders.iter().map(|f| f.tracks).sum();
            let folders = settings.library_folders.len();
            message.push(format!(
                "kōan stops scanning {} and forgets its {} tracks. Your files are not touched.",
                if folders == 1 {
                    "1 folder".to_owned()
                } else {
                    format!("{folders} folders")
                },
                count(tracks)
            ));
        }
        message.push("config.toml is kept beside itself as config.toml.bak. config.local.toml, which holds passwords and keys, is emptied. The output kōan is playing through stays in use until it next opens one.".into());
        let model = self.ctx.model.clone();
        let sharing = self.ctx.sharing.clone();
        let sheets = self.ctx.sheets.clone();
        // A config.toml kept in a dotfiles repository is a link into it, and
        // emptying it empties the file there, for every machine sharing it.
        let linked = cx.background_executor().spawn(async {
            let path = koan_core::config::config_file_path();
            std::fs::symlink_metadata(&path)
                .is_ok_and(|m| m.file_type().is_symlink())
                .then(|| std::fs::canonicalize(&path).ok())
                .flatten()
        });
        window
            .spawn(cx, async move |cx| {
                if let Some(target) = linked.await {
                    message.push(format!(
                        "config.toml is a link to {}. Emptying it empties that file, for every machine that shares it.",
                        homely(&target.to_string_lossy())
                    ));
                }
                let sheet = Sheet::new("Reset kōan to its defaults?")
                    .message(message.join("\n\n"))
                    .action(
                        Action::new("Reset to Defaults", move |_, _, cx| {
                            let sharing = sharing.clone();
                            model.update(cx, |model, cx| {
                                model.reset(sharing, crate::setup::after_reset, cx)
                            })
                        })
                        .destructive(),
                    );
                cx.update(|window, cx| sheets.update(cx, |s, cx| s.show(sheet, window, cx)))
                    .ok();
            })
            .detach();
    }

    fn rebuild(&self, window: &mut Window, cx: &mut App) {
        let model = self.ctx.model.clone();
        let sheet = Sheet::new("Rebuild the library index?")
            .message("The next scan re-reads every file and the next sync walks the whole server, and each track takes what its tags say now. Play history, playlists, favourites and lyrics are kept. A track nothing finds again goes. Your music files are not touched.")
            .action(
                Action::new("Rebuild Index", move |_, _, cx| {
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
                            // Cut in the middle, so the folder's own name at
                            // the end stays in view however deep it lies.
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis_middle()
                            .child(homely(&folder.path)),
                    )
                    .child(text(
                        format!("{} tracks", count(folder.tracks)),
                        TypeRole::Fine,
                        Tone::Muted,
                        &look,
                    ))
                    .child(
                        Button::icon_only(
                            ("remove-folder", index),
                            Icon::Remove,
                            label("Stop scanning this folder"),
                        )
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
                    .help(crate::theme::label(
                        "Re-read every file's tags, ignoring the scan cache",
                    ))
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
                    .on_click({
                        let entity = entity.clone();
                        move |_, window, cx| entity.update(cx, |this, cx| this.rebuild(window, cx))
                    })
                    .into_any_element(),
            ],
            &look,
        );
        let start_over = buttons(
            vec![
                Button::new("set-up", ButtonKind::Bordered)
                    .label("Set Up kōan…")
                    .on_click(|_, _, cx| crate::setup::open(cx))
                    .into_any_element(),
                Button::new("reset", ButtonKind::Bordered)
                    .label("Reset to Defaults…")
                    .destructive(true)
                    .disabled(whole)
                    .on_click(move |_, window, cx| {
                        entity.update(cx, |this, cx| this.reset(window, cx))
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
                section(
                    Some("Start over"),
                    vec![start_over],
                    Some("The setup guide walks through choosing your music, sharing it and playback again. Resetting puts every setting back to its default first.".into()),
                    &look,
                ),
            ],
        )
    }
}

/// Asks for folders to add, and adds and scans them. The task is the
/// question: dropped, it is not answered.
pub(crate) fn choose_folders(
    model: Entity<crate::settings::model::SettingsModel>,
    cx: &mut App,
) -> Task<()> {
    let chosen = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: true,
        prompt: Some("Add".into()),
    });
    cx.spawn(async move |cx| {
        let Ok(Ok(Some(paths))) = chosen.await else {
            return;
        };
        let paths = paths
            .into_iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        model.update(cx, |model, cx| model.add_folders(paths, cx));
    })
}

/// Asks before a folder goes: kept rows stay in the library, forgotten ones
/// go, and its files are not touched either way.
pub(crate) fn ask_remove(ctx: &Ctx, path: String, tracks: u64, window: &mut Window, cx: &mut App) {
    let keep = {
        let model = ctx.model.clone();
        let path = path.clone();
        move |_: &[String], _: &mut Window, cx: &mut App| {
            model.update(cx, |model, cx| model.remove_folder(path.clone(), false, cx))
        }
    };
    let forget = {
        let model = ctx.model.clone();
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
    ctx.sheets
        .update(cx, |sheets, cx| sheets.show(sheet, window, cx));
}

/// A path as people write it: the home directory as `~`.
pub(crate) fn homely(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && path.starts_with(&home) => {
            let rest = &path[home.len()..];
            if rest.is_empty() || rest.starts_with('/') {
                format!("~{rest}")
            } else {
                path.to_owned()
            }
        }
        _ => path.to_owned(),
    }
}
