//! The storybook: every component in every state, each beside the call that drew it.
//!
//! ```sh
//! cargo run -p koan-kit-web --example storybook --features storybook              # writes storybook/index.html
//! cargo run -p koan-kit-web --example storybook --features storybook -- --serve   # serves it live on :4580
//! ```
//!
//! The written page is what ui.koan.rocks serves: static, with no Datastar. Served, the same page loads Datastar and
//! talks to the stub handlers here, which wait a moment so a busy button, a question, a preview and a live search can
//! be watched. Serving reads the compiled stylesheet, `storybook/out.css`, which `mise run storybook` makes.

use std::fmt::Write as _;
use std::time::Duration;

use askama_datastar::{Html, events, require_datastar_on_post, script};
use axum::Router;
use axum::extract::Query;
use axum::http::header;
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use koan_kit_web as kit;
use serde_json::Value;

const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/storybook");

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some("--serve") {
        let path = format!("{DIR}/index.html");
        std::fs::write(&path, kit::storybook::page(false).as_str())?;
        println!("wrote {path}");
        return Ok(());
    }
    let port: u16 = args
        .next()
        .and_then(|port| port.parse().ok())
        .unwrap_or(4580);
    let app = Router::new()
        .route("/stub/slow", post(slow))
        .layer(middleware::from_fn(require_datastar_on_post))
        .route("/", get(page))
        .route("/out.css", get(stylesheet))
        .route("/geist-mono.woff2", get(kit::font::serve))
        .route("/datastar.js", get(script::serve))
        .route("/stub/flash", get(flash))
        .route("/stub/failed", get(failed))
        .route("/stub/moved", get(moved))
        .route("/stub/place", get(place))
        .route("/stub/preview", get(preview))
        .route("/stub/status", get(status))
        .route("/stub/files", get(files))
        .route("/stub/search", get(search));
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    println!("the storybook, live, on http://127.0.0.1:{port}");
    axum::serve(listener, app).await
}

async fn page() -> Html {
    kit::storybook::page(true)
}

async fn stylesheet() -> Response {
    match tokio::fs::read_to_string(format!("{DIR}/out.css")).await {
        Ok(css) => ([(header::CONTENT_TYPE, "text/css; charset=utf-8")], css).into_response(),
        Err(_) => "no storybook/out.css: run `mise run storybook` in koan-kit-web/".into_response(),
    }
}

/// A change that takes a moment, then says so and closes the question that asked for it.
async fn slow() -> Response {
    tokio::time::sleep(Duration::from_millis(900)).await;
    events([
        kit::Flash::ok("Done.").patch().into(),
        kit::Confirm::close().into(),
    ])
}

async fn flash() -> Response {
    events([kit::Flash::ok("Saved.").patch().into()])
}

async fn moved() -> Response {
    events([kit::Flash::ok("Moved 3 images.")
        .link("#content", "open that album")
        .patch()
        .into()])
}

/// A page read in place: its answer, a moment later, says so.
async fn place() -> Response {
    tokio::time::sleep(Duration::from_millis(900)).await;
    events([kit::Flash::ok("Loaded in place.").patch().into()])
}

async fn failed() -> Response {
    events([kit::Flash::bad("The server refused the token: 401.")
        .dismiss("dismiss")
        .patch()
        .into()])
}

/// The server's preview of a change: what each file is called now, and what it will be.
async fn preview() -> Response {
    tokio::time::sleep(Duration::from_millis(700)).await;
    let plan = kit::Pairs::new()
        .pair("01 track.flac", "01 Held on the Tips of Fingers.flac")
        .pair("02 track.flac", "02 Fluffy.flac")
        .pair("03 track.flac", "03 Peepy.flac");
    events(kit::Confirm::preview(plan))
}

async fn status() -> Response {
    tokio::time::sleep(Duration::from_millis(1200)).await;
    events([kit::Status::ok("connected")
        .id("story-status")
        .patch()
        .into()])
}

/// The disclosure that loads, answered with itself: open, and holding what it read.
async fn files() -> Response {
    tokio::time::sleep(Duration::from_millis(800)).await;
    let list = Html::trusted(
        r#"<ul class="rows"><li>01 Held on the Tips of Fingers.flac</li><li>02 Fluffy.flac</li></ul>"#,
    );
    events([kit::Disclosure::new("files")
        .body(list)
        .open(true)
        .id("story-files")
        .patch()
        .into()])
}

/// A live search's answer: the albums whose name holds the query.
async fn search(Query(query): Query<serde_json::Map<String, Value>>) -> Response {
    let words = query
        .get("q")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_lowercase();
    let albums = [
        "Polar Bear — Held on the Tips of Fingers",
        "Polar Bear — Peepers",
        "Phace — Impulse",
        "Burial — Untrue",
    ];
    let mut rows = String::new();
    for album in albums
        .iter()
        .filter(|album| !words.is_empty() && album.to_lowercase().contains(&words))
    {
        // Content escapes words as it writes them.
        let _ = write!(rows, "<li>{}</li>", kit::Content::from(*album));
    }
    events([Html::trusted(format!(
        "<ul id=\"story-results\" class=\"rows\">{rows}</ul>"
    ))
    .patch()
    .into()])
}
