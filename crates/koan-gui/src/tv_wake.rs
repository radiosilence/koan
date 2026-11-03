//! Setting an Apple TV up to be woken from the account's devices, as the Mac
//! app's `TvWake` does. The TV is paired once, the way the Remote app pairs:
//! it shows a four-digit code and this device types it. The server keeps the
//! pairing for the account, and from then on choosing the TV on any of the
//! account's devices on its network turns it on and opens kōan there. The TV
//! hangs up about half a minute after asking, so the code is asked for only
//! once it is on screen.
//!
//! Offered from the TV's row in Play on ("set up waking…"), and once after
//! a TV is signed in from here (Settings → Devices), as soon as it links,
//! as the Mac's `SignedInHere` does; "stop waking" in Play on undoes it.

use koan_kit::app::*;

use crate::components::sheet as ask;
use crate::components::{Button, ButtonKind, FieldState, InputEvent, Spinner, TextField};
use crate::engine::{engine, reported};
use crate::paint::Paint;
use crate::theme::{Space, Tone, TypeRole};

/// How long after a sign-in from here the TV's linking is still taken as
/// part of setting it up: later, it is no longer what the person is doing.
const OFFER_WINDOW: std::time::Duration = std::time::Duration::from_secs(300);

/// A device just signed in from here: if it is an Apple TV, waking is offered
/// once it links. Known by the id it links with, or by its name from a
/// server that does not say the id, since two televisions are often called
/// the same.
struct SignedInHere {
    id: Option<String>,
    name: String,
    at: std::time::Instant,
}

impl Global for SignedInHere {}

/// Settings approved a device's sign-in.
pub fn signed_in(id: Option<String>, name: String, cx: &mut App) {
    cx.set_global(SignedInHere {
        id,
        name,
        at: std::time::Instant::now(),
    });
}

/// Watches the devices for a TV signed in from here arriving, and offers
/// waking it once.
pub fn install(cx: &mut App) {
    let devices = cx.global::<crate::mirror::Mirror>().devices.clone();
    cx.observe(&devices, |devices, cx| {
        let Some(signed) = cx.try_global::<SignedInHere>() else {
            return;
        };
        if signed.at.elapsed() > OFFER_WINDOW {
            cx.remove_global::<SignedInHere>();
            return;
        }
        let arrived = devices.read(cx).0.devices.iter().find(|d| {
            d.tv_pairable
                && !d.tv_paired
                && d.awake
                && match &signed.id {
                    Some(id) => d.id == *id,
                    None => d.name == signed.name,
                }
        });
        let Some(tv) = arrived.map(|d| (d.id.clone(), d.name.clone())) else {
            return;
        };
        cx.remove_global::<SignedInHere>();
        offer(tv.0, tv.1, cx);
    })
    .detach();
}

/// "Wake the TV from here?", in the main window: set up opens the pairing.
fn offer(id: String, name: String, cx: &mut App) {
    let Some(main) = crate::app::main_window(cx) else {
        return;
    };
    main.update(cx, |_, window, cx| {
        ask::confirm(
            ask::Confirm {
                title: format!("wake {name} from here?").into(),
                verb: "set up".into(),
                destructive: false,
                lines: vec![
                    format!(
                        "Choosing {name} from any of your devices on its network can turn the TV on and open kōan. {name} shows a code to type here, once."
                    )
                    .into(),
                ],
                note: None,
            },
            window,
            cx,
            move |cx| {
                let (id, name) = (id.clone(), name.clone());
                if let Some(main) = crate::app::main_window(cx) {
                    main.update(cx, |_, window, cx| open(id, name, window, cx)).ok();
                }
            },
        );
    })
    .ok();
}

/// Opens the sheet for the TV `id`, called `name`.
pub fn open(id: String, name: String, window: &mut Window, cx: &mut App) {
    let sheet = cx.new(|cx| TvWake::new(id, name, window, cx));
    ask::present(sheet, window, cx);
}

/// Asks, then forgets the TV's pairing for all the account's devices:
/// getting it back means pairing at the TV again.
pub fn stop(id: String, name: String, window: &mut Window, cx: &mut App) {
    ask::confirm(
        ask::Confirm {
            title: "stop waking".into(),
            verb: "stop waking".into(),
            destructive: true,
            lines: vec![
                format!("Choosing {name} no longer turns it on, from any of your devices.").into(),
            ],
            note: Some(
                "To wake it from here again, set up waking and type the code it shows.".into(),
            ),
        },
        window,
        cx,
        move |cx| {
            let id = id.clone();
            reported(cx, "stop waking", move |e| e.forget_tv_pairing(id));
        },
    );
}

#[derive(Clone, PartialEq)]
enum Phase {
    Asking,
    Entering,
    Pairing,
    Done,
    Failed(String),
}

pub struct TvWake {
    id: String,
    name: String,
    phase: Phase,
    /// The name the TV asked under: the screen the code should be on.
    asked: Option<String>,
    code: Entity<FieldState>,
    _task: Option<Task<()>>,
    _code: Subscription,
    _released: Subscription,
}

impl TvWake {
    fn new(id: String, name: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let code = cx.new(|cx| FieldState::new(window, cx).placeholder("0000"));
        let subscription = cx.subscribe_in(&code, window, |this, _, event: &InputEvent, _, cx| {
            match event {
                // Four digits typed is the whole code.
                InputEvent::Change if this.digits(cx).len() == 4 => this.finish(cx),
                InputEvent::Change => cx.notify(),
                InputEvent::PressEnter { .. } => this.finish(cx),
                _ => {}
            }
        });
        // However the sheet goes (closed, or replaced by another), a pairing
        // not finished is called off, so the TV puts its code away.
        let released = cx.on_release(|this: &mut Self, cx| {
            if this.phase != Phase::Done {
                engine(cx).cancel_tv_pairing();
            }
        });
        let mut this = Self {
            id,
            name,
            phase: Phase::Asking,
            asked: None,
            code,
            _task: None,
            _code: subscription,
            _released: released,
        };
        this.begin(window, cx);
        this
    }

    fn digits(&self, cx: &App) -> String {
        self.code
            .read(cx)
            .value()
            .chars()
            .filter(char::is_ascii_digit)
            .collect()
    }

    /// Asks the TV to show a code: again for "try again".
    fn begin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.phase = Phase::Asking;
        self.code
            .update(cx, |code, cx| code.set_value("", window, cx));
        let begun = engine(cx).begin_tv_pairing(self.id.clone());
        self._task = Some(cx.spawn_in(window, async move |this, cx| {
            let begun = begun.await;
            this.update_in(cx, |this, window, cx| {
                match begun {
                    Ok(asked) => {
                        this.asked = Some(asked);
                        this.phase = Phase::Entering;
                        this.code.update(cx, |code, cx| code.focus(window, cx));
                    }
                    Err(e) => this.phase = Phase::Failed(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn finish(&mut self, cx: &mut Context<Self>) {
        let pin = self.digits(cx);
        if self.phase != Phase::Entering || pin.len() != 4 {
            return;
        }
        self.phase = Phase::Pairing;
        let finished = engine(cx).finish_tv_pairing(pin);
        self._task = Some(cx.spawn(async move |this, cx| {
            let finished = finished.await;
            this.update(cx, |this, cx| {
                this.phase = match finished {
                    Ok(()) => Phase::Done,
                    Err(e) => Phase::Failed(e.to_string()),
                };
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        ask::close(cx);
    }
}

impl Render for TvWake {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Paint::of(window, cx);
        let look = &p.look;
        let done = self.phase == Phase::Done;
        let close = Button::new("tv-wake-close", ButtonKind::Standard)
            .label(if done { "done" } else { "cancel" })
            .on_click(cx.listener(|this, _, _, cx| this.close(cx)));
        let name = self.name.clone();
        let waiting = |text: String| {
            div()
                .flex()
                .items_center()
                .gap(look.space(Space::S))
                .child(Spinner::new(look.font_size(TypeRole::Control)))
                .child(ask::note(text, Tone::Muted, look))
                .into_any_element()
        };
        let content = match &self.phase {
            Phase::Asking => waiting(format!("Asking {name} for a code…")),
            Phase::Pairing => waiting("Pairing…".into()),
            Phase::Entering => {
                let on = self.asked.clone().unwrap_or_else(|| name.clone());
                let ready = self.digits(cx).len() == 4;
                div()
                    .flex()
                    .flex_col()
                    .gap(look.space(Space::M))
                    .child(ask::note(
                        format!("Type the code on {on}. It waits about half a minute."),
                        Tone::Muted,
                        look,
                    ))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(look.space(Space::M))
                            .child(div().flex_1().child(TextField::new(&self.code)))
                            .child(
                                Button::new("tv-wake-pair", ButtonKind::Prominent)
                                    .label("pair")
                                    .disabled(!ready)
                                    .on_click(cx.listener(|this, _, _, cx| this.finish(cx))),
                            ),
                    )
                    .into_any_element()
            }
            Phase::Done => ask::note(
                format!(
                    "Choosing {name} from any of your devices on its network now turns it on and opens kōan."
                ),
                Tone::Muted,
                look,
            )
            .into_any_element(),
            Phase::Failed(why) => div()
                .flex()
                .flex_col()
                .gap(look.space(Space::M))
                .child(ask::note(why.clone(), Tone::Bad, look))
                .child(
                    Button::new("tv-wake-retry", ButtonKind::Bordered)
                        .label("try again")
                        .on_click(cx.listener(|this, _, window, cx| this.begin(window, cx))),
                )
                .into_any_element(),
        };
        ask::panel("wake from sleep", look)
            .child(ask::head("Wake from Sleep", vec![close.into_any_element()], look))
            .child(
                ask::body("tv-wake", look)
                    .flex()
                    .flex_col()
                    .gap(look.space(Space::L))
                    .child(ask::note(self.name.clone(), Tone::Ink, look))
                    .child(content)
                    .child(ask::note(
                        "kōan pairs with the TV as the Remote app does. The TV lists it under Settings › Remotes and Devices, and \u{201C}stop waking\u{201D} in Play on undoes it for all your devices.",
                        Tone::Muted,
                        look,
                    )),
            )
    }
}
