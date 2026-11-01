//! Everything known about a track, over the page: its tags, each source it is
//! made from with that source's own fields, and its ReplayGain. A dense view,
//! in `meta` and `fine`, names leading and values trailing.

use koan_ffi::{InfoField, TrackDetails};
use koan_kit::raw::prelude::*;
use koan_kit::raw::*;

use super::format;
use super::tokens::{Paint, Type, Typeset};
use super::{Dismiss, Lib};
use crate::theme::Space;

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

fn field(p: &Paint, name: impl Into<SharedString>, value: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .justify_between()
        .gap(p.space(Space::L))
        .child(div().typeset(p, Type::Meta, p.muted).child(name.into()))
        .child(
            div()
                .typeset(p, Type::Meta, p.ink)
                .text_right()
                .child(value.into()),
        )
}

fn section(p: &Paint, title: impl Into<SharedString>, fields: Vec<Div>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(p.space(Space::Xs))
        .pt(p.space(Space::L))
        .child(div().typeset(p, Type::Fine, p.ink).child(title.into()))
        .children(fields)
}

fn fields(p: &Paint, list: &[InfoField]) -> Vec<Div> {
    list.iter()
        .map(|f| field(p, f.name.clone(), f.value.clone()))
        .collect()
}

impl Render for TrackInfo {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = &Paint::of(window, cx);
        let body = match &self.details {
            Some(d) => {
                let t = &d.track;
                let mut tags = vec![
                    field(p, "title", t.title.clone()),
                    field(p, "artist", t.artist_name.clone()),
                    field(p, "album", t.album_title.clone()),
                    field(p, "album artist", t.album_artist_name.clone()),
                ];
                if let Some(n) = t.track_number {
                    tags.push(field(p, "track", n.to_string()));
                }
                if let Some(n) = t.disc {
                    tags.push(field(p, "disc", n.to_string()));
                }
                if let Some(g) = &t.genre {
                    tags.push(field(p, "genre", g.clone()));
                }
                tags.push(field(p, "length", format::duration(t.duration_ms)));
                if let Some(q) = format::quality(t) {
                    tags.push(field(p, "format", q));
                }
                if let Some(path) = &t.path {
                    tags.push(field(p, "file", path.clone()));
                }
                if let Some(uid) = &d.uid {
                    tags.push(field(p, "uid", uid.clone()));
                }
                let mut out = div().flex().flex_col().child(section(p, "tags", tags));
                for source in &d.sources {
                    out = out.child(section(
                        p,
                        source.kind.to_lowercase(),
                        fields(p, &source.fields),
                    ));
                }
                if !d.replay_gain.is_empty() {
                    out = out.child(section(p, "replaygain", fields(p, &d.replay_gain)));
                }
                out.into_any_element()
            }
            None if self.missing => div()
                .typeset(p, Type::Control, p.muted)
                .child("this track isn't in the library any more")
                .into_any_element(),
            None => div().into_any_element(),
        };
        // Over the page, on a flat dim; a click outside closes it.
        div()
            .id("track-info-scrim")
            .key_context("Library")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::close))
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(p.dim())
            .on_click(cx.listener(|this, _, _, cx| {
                this.lib.nav.update(cx, |nav, cx| nav.close_info(cx));
            }))
            .child(
                div()
                    .id("track-info")
                    .w(px(480.0))
                    .max_h(relative(0.8))
                    .overflow_y_scroll()
                    .p(p.space(Space::L))
                    .bg(p.bg)
                    .border_1()
                    .border_color(p.rule)
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .typeset(p, Type::TitleSmall, p.strong)
                            .child("track info"),
                    )
                    .child(body),
            )
    }
}
