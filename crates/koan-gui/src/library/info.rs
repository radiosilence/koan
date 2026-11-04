//! Everything the library knows about a track, over the page, as each of its
//! sources says it: the file's tags, the server's entry, the ReplayGain, and
//! the library's own ids. Dense, for reading a value off.

use koan_kit::app::*;
use koan_runtime::{InfoField, TrackDetails};

use super::{Dismiss, Lib};
use crate::components::sheet as ask;
use crate::components::{Button, ButtonKind, DetailGroup, form_text};
use crate::theme::{Look, Space, Tone, TypeRole};

pub struct TrackInfo {
    lib: Lib,
    details: Option<TrackDetails>,
    missing: bool,
    focus: FocusHandle,
    _load: Task<()>,
}

impl TrackInfo {
    pub fn new(lib: Lib, track: i64, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let engine = lib.engine.clone();
        let load = cx.spawn(async move |this, cx| {
            let details = engine.track_details(track).await;
            this.update(cx, |this, cx| {
                match details {
                    Ok(Some(details)) => this.details = Some(details),
                    Ok(None) => this.missing = true,
                    Err(e) => {
                        log::warn!("track {track}: {e}");
                        this.missing = true;
                    }
                }
                cx.notify();
            })
            .ok();
        });
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            lib,
            details: None,
            missing: false,
            focus,
            _load: load,
        }
    }

    fn close(&mut self, _: &Dismiss, _: &mut Window, cx: &mut Context<Self>) {
        self.lib.nav.update(cx, |nav, cx| nav.close_info(cx));
    }
}

/// A source's fields under its kind, as the Mac names them.
fn group(title: &str, fields: &[InfoField]) -> DetailGroup {
    DetailGroup::new(title.to_string()).rows(
        fields
            .iter()
            .map(|f| (f.name.to_lowercase(), f.value.clone())),
    )
}

impl Render for TrackInfo {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = &Look::of(window, cx);
        let mut body = ask::body("track-info-body", look).pt(look.space(Space::M));
        match &self.details {
            Some(d) => {
                body = body.child(form_text(
                    d.track.title.clone(),
                    TypeRole::Body,
                    Tone::Strong,
                    look,
                ));
                for source in &d.sources {
                    let kind = if source.kind == "server" {
                        "server"
                    } else {
                        "file"
                    };
                    body = body.child(group(kind, &source.fields));
                }
                // A server's track played here is kept on disk with no file
                // source of its own: where the copy is.
                if let Some(path) = &d.track.path
                    && d.sources.iter().all(|s| s.kind == "server")
                {
                    body = body.child(DetailGroup::new("on this device").row("path", path.clone()));
                }
                if !d.replay_gain.is_empty() {
                    body = body.child(group("replaygain", &d.replay_gain));
                }
                let mut ids = DetailGroup::new("library").row("track id", d.track.id.to_string());
                if let Some(uid) = &d.uid {
                    ids = ids.row("uid", uid.clone());
                }
                body = body.child(ids);
            }
            None if self.missing => {
                body = body.child(ask::note(
                    "This track is no longer in the library.",
                    Tone::Muted,
                    look,
                ));
            }
            None => {}
        }
        let done = Button::new("track-info-done", ButtonKind::Standard)
            .label("done")
            .on_click(cx.listener(|this, _, _, cx| {
                this.lib.nav.update(cx, |nav, cx| nav.close_info(cx));
            }));
        // Over the page; a click outside closes it.
        ask::scrim("track-info-scrim", look)
            .key_context("Library")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::close))
            .on_click(cx.listener(|this, _, _, cx| {
                this.lib.nav.update(cx, |nav, cx| nav.close_info(cx));
            }))
            .child(crate::components::Rise::new(
                ask::panel("track info", look)
                    .id("track-info")
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(ask::head("track info", vec![done.into_any_element()], look))
                    .child(body),
            ))
    }
}
