use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use notify_debouncer_full::{
    DebouncedEvent, Debouncer, FileIdMap, new_debouncer, notify::RecursiveMode,
};
use thiserror::Error;

use crate::db::connection::Database;
use crate::db::queries;
use crate::index::metadata;

#[derive(Debug, Error)]
pub enum WatcherError {
    #[error("notify error: {0}")]
    Notify(#[from] notify::Error),
}

/// Watches library folders for changes and updates the database.
///
/// Uses a channel internally because rusqlite::Connection isn't Sync —
/// the notify callback sends events to a dedicated DB writer thread.
pub struct LibraryWatcher {
    _debouncer: Debouncer<notify::RecommendedWatcher, FileIdMap>,
    _writer_thread: std::thread::JoinHandle<()>,
}

impl LibraryWatcher {
    pub fn start(folders: &[PathBuf], db: Database) -> Result<Self, WatcherError> {
        let (tx, rx) = mpsc::channel::<Vec<DebouncedEvent>>();

        let mut debouncer = new_debouncer(
            Duration::from_millis(500),
            None,
            move |result: notify_debouncer_full::DebounceEventResult| match result {
                Ok(events) => {
                    let _ = tx.send(events);
                }
                Err(errors) => {
                    for e in errors {
                        log::error!("watch error: {}", e);
                    }
                }
            },
        )?;

        for folder in folders {
            if folder.exists() {
                debouncer.watch(folder, RecursiveMode::Recursive)?;
                log::info!("watching: {}", folder.display());
            }
        }

        // DB writer thread — processes events sequentially.
        let writer_thread = std::thread::Builder::new()
            .name("koan-watcher-db".into())
            .spawn(move || {
                while let Ok(events) = rx.recv() {
                    for event in &events {
                        handle_event(&db, event);
                    }
                }
            })
            .expect("failed to spawn watcher db thread");

        Ok(Self {
            _debouncer: debouncer,
            _writer_thread: writer_thread,
        })
    }
}

fn handle_event(db: &Database, event: &DebouncedEvent) {
    use notify::EventKind;

    match &event.kind {
        EventKind::Create(_) | EventKind::Modify(_) => {
            for path in &event.paths {
                if metadata::is_audio_file(path) && path.exists() {
                    match metadata::read_metadata(path) {
                        Ok(meta) => match queries::upsert_track(&db.conn, &meta) {
                            Ok(track_id) => {
                                let _ = queries::update_scan_cache(
                                    &db.conn,
                                    meta.path.as_deref().unwrap_or(""),
                                    meta.mtime.unwrap_or(0),
                                    meta.size_bytes.unwrap_or(0),
                                    track_id,
                                );
                                log::debug!("indexed: {}", path.display());
                            }
                            Err(e) => log::error!("db error indexing {}: {}", path.display(), e),
                        },
                        Err(e) => log::warn!("metadata error {}: {}", path.display(), e),
                    }
                }
            }
        }
        EventKind::Remove(_) => {
            for path in &event.paths {
                if metadata::is_audio_file(path) {
                    let path_str = path.to_string_lossy();
                    if let Err(e) = queries::remove_track_by_path(&db.conn, &path_str) {
                        log::error!("db error removing {}: {}", path.display(), e);
                    } else {
                        log::debug!("removed: {}", path.display());
                    }
                }
            }
        }
        _ => {}
    }
}
