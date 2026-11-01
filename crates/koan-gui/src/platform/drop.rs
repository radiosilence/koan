//! Files from outside the app: dropped from Finder or a file manager, named
//! on the command line, or handed over by a second launch.
//!
//! They are indexed where they lie and queued, as the Mac app's
//! `PlayerModel.importFiles` does: a drop is "play this", and giving the files
//! library rows is what lets organize move them into the music tree later, on
//! purpose and with a preview. Folders are walked, so dropping a rip queues the
//! album. A drop onto a playlist adds to the playlist instead.
//!
//! A drop target takes `ExternalPaths`:
//!
//! ```ignore
//! div()
//!     .drag_over::<ExternalPaths>(|style, _, _, _| style /* the drop highlight */)
//!     .on_drop(move |paths: &ExternalPaths, _, cx| {
//!         drop::queue_files(&engine, paths.paths().to_vec(), cx)
//!     })
//! ```

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::{App, AsyncApp, Global};
use koan_ffi::{ImportSummary, KoanEngine, TrackSet};

/// What the Mac app says when a drop held nothing to play.
pub const NOTHING_TO_PLAY: &str = "Nothing there kōan can play.";

/// Imports under way, and what the last one had to say. A folder of a few
/// hundred files takes long enough that a drop with no sign of life reads as
/// one that missed, so the shell shows `running` as an activity ("Adding
/// dropped files") and the problem as the app's error line; it follows this
/// with `cx.observe_global::<Imports>`, reading `running` and calling
/// `Imports::take_problem`.
#[derive(Default)]
pub struct Imports {
    pub running: usize,
    problem: Option<String>,
}

impl Global for Imports {}

impl Imports {
    /// What the last import had to say, once. Writes the global only when
    /// there is something to take: any write notifies its observers, so an
    /// observer that cleared nothing would wake itself for ever.
    pub fn take_problem(cx: &mut App) -> Option<String> {
        cx.try_global::<Imports>()?.problem.as_ref()?;
        cx.global_mut::<Imports>().problem.take()
    }
}

/// Where a finished import's tracks go.
enum Into {
    Queue,
    Playlist(i64),
}

/// What an import found, as the person is told it: nothing playable is a
/// problem and nothing goes anywhere; otherwise the first file that failed,
/// if any, while the rest still go where they were dropped.
fn outcome(summary: &ImportSummary) -> Option<String> {
    if summary.track_ids.is_empty() {
        Some(NOTHING_TO_PLAY.to_string())
    } else {
        summary.errors.first().cloned()
    }
}

/// Index `paths` and append them to the queue. Nothing starts playing.
pub fn queue_files(engine: &Arc<KoanEngine>, paths: Vec<PathBuf>, cx: &mut App) {
    import(engine, paths, Into::Queue, cx);
}

/// Index `paths` and add them to the end of a playlist.
pub fn add_files_to_playlist(
    engine: &Arc<KoanEngine>,
    playlist_id: i64,
    paths: Vec<PathBuf>,
    cx: &mut App,
) {
    import(engine, paths, Into::Playlist(playlist_id), cx);
}

fn import(engine: &Arc<KoanEngine>, paths: Vec<PathBuf>, into: Into, cx: &mut App) {
    let paths: Vec<String> = paths
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    if paths.is_empty() {
        return;
    }
    cx.default_global::<Imports>().running += 1;
    let engine = engine.clone();
    cx.spawn(async move |cx: &mut AsyncApp| {
        let problem = match engine.clone().import_files(paths).await {
            // As the Mac app says it: an import that fails is a drop with
            // nothing to play in it. The log has the reason.
            Err(e) => {
                log::warn!("import: {e}");
                Some(NOTHING_TO_PLAY.to_string())
            }
            Ok(summary) => {
                let problem = outcome(&summary);
                let placed = match into {
                    _ if summary.track_ids.is_empty() => Ok(()),
                    Into::Queue => engine.add_to_queue(summary.track_ids).await.map(drop),
                    Into::Playlist(id) => engine
                        .add_to_playlist(
                            id,
                            vec![TrackSet::Tracks {
                                ids: summary.track_ids,
                            }],
                        )
                        .await
                        .map(drop),
                };
                placed.err().map(|e| e.to_string()).or(problem)
            }
        };
        if let Some(problem) = &problem {
            log::warn!("import: {problem}");
        }
        cx.update(|cx| {
            let imports = cx.default_global::<Imports>();
            imports.running = imports.running.saturating_sub(1);
            if problem.is_some() {
                imports.problem = problem;
            }
        });
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(track_ids: Vec<i64>, errors: &[&str]) -> ImportSummary {
        ImportSummary {
            track_ids,
            added: 0,
            updated: 0,
            errors: errors.iter().map(|e| e.to_string()).collect(),
        }
    }

    #[test]
    fn nothing_playable_says_so() {
        assert_eq!(
            outcome(&summary(vec![], &["a.txt: not audio"])).as_deref(),
            Some(NOTHING_TO_PLAY)
        );
    }

    #[test]
    fn a_partial_import_reports_its_first_failure() {
        assert_eq!(
            outcome(&summary(
                vec![1, 2],
                &["b.flac: unreadable", "c.flac: unreadable"]
            ))
            .as_deref(),
            Some("b.flac: unreadable")
        );
    }

    #[test]
    fn a_clean_import_says_nothing() {
        assert_eq!(outcome(&summary(vec![1], &[])), None);
    }
}
