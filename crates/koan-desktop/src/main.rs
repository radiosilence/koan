use futures_channel::mpsc;
use gpui_kit::*;
use koan_desktop::platform;
use platform::instance::{self, Launch, Opened};

struct Shell;

impl Render for Shell {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().key_context(platform::actions::CONTEXT)
    }
}

fn main() {
    platform::logging::init();
    let paths = instance::argv_paths();
    let (opened, opening) = mpsc::unbounded();
    match instance::claim(&paths) {
        Ok(Launch::Handed) => return,
        Ok(Launch::First(server)) => server.serve(opened.clone()),
        // Running alone is better than not running.
        Err(e) => log::warn!("instance: {e}; not answering later launches"),
    }
    if !paths.is_empty() {
        let _ = opened.unbounded_send(Opened::Paths(paths));
    }

    let app = gpui_kit::application();
    app.on_open_urls(move |urls| {
        for next in instance::sort_urls(urls) {
            let _ = opened.unbounded_send(next);
        }
    });
    app.run(|cx| {
        gpui_kit::init(cx);
        cx.spawn(async move |cx: &mut AsyncApp| {
            let engine = match koan_ffi::KoanEngine::open_desktop().await {
                Ok(engine) => engine,
                Err(e) => {
                    log::error!("could not open the engine: {e}");
                    cx.update(|cx| cx.quit());
                    return;
                }
            };
            cx.update(|cx| {
                platform::init(&engine, opening, cx);
                gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| cx.new(|_| Shell))
                    .expect("failed to open the window");
            });
        })
        .detach();
    });
}
