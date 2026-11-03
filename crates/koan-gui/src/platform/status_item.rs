//! The menu bar item kōan keeps while it runs with its window closed, as the
//! Mac app's `MenuBarExtra`: the ensō, and a menu of what is playing, play
//! and pause, next, the way back to the window, and quit. GPUI has no status
//! item, so this is AppKit's `NSStatusItem`; its menu is rebuilt from the
//! mirror's Playback slice only when what it shows changes.

use futures::StreamExt as _;
use futures_channel::mpsc::{UnboundedSender, unbounded};
use koan_kit::app::*;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{AnyThread, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSImage, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem, NSVariableStatusItemLength,
};
use objc2_foundation::{NSData, NSSize, NSString};

use super::actions::{Next, TogglePlayPause};
use crate::mirror::{Mirror, Playback};

/// What an item of the menu asks for.
#[derive(Clone, Copy)]
enum Command {
    PlayPause,
    Next,
    Open,
    Quit,
}

thread_local! {
    /// Where the menu's target sends what was chosen: to the task that
    /// answers it on GPUI's side. The main thread's alone.
    static CHOSEN: std::cell::RefCell<Option<UnboundedSender<Command>>> =
        const { std::cell::RefCell::new(None) };
}

fn choose(command: Command) {
    CHOSEN.with(|chosen| {
        if let Some(chosen) = &*chosen.borrow() {
            chosen.unbounded_send(command).ok();
        }
    });
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and the class keeps
    // no state of its own.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "KoanStatusTarget"]
    struct Target;

    impl Target {
        #[unsafe(method(playPause:))]
        fn play_pause(&self, _sender: Option<&AnyObject>) {
            choose(Command::PlayPause);
        }

        #[unsafe(method(next:))]
        fn next(&self, _sender: Option<&AnyObject>) {
            choose(Command::Next);
        }

        #[unsafe(method(open:))]
        fn open(&self, _sender: Option<&AnyObject>) {
            choose(Command::Open);
        }

        #[unsafe(method(quit:))]
        fn quit(&self, _sender: Option<&AnyObject>) {
            choose(Command::Quit);
        }
    }
);

/// What the menu shows: built again only when this changes.
#[derive(Clone, PartialEq, Default)]
struct Shown {
    title: Option<String>,
    artist: Option<String>,
    playing: bool,
}

/// The item, while it is in the menu bar; dropped, it leaves it.
pub struct StatusItem {
    item: Retained<NSStatusItem>,
    menu: Retained<NSMenu>,
    target: Retained<Target>,
    shown: Option<Shown>,
    _playback: Subscription,
    _answers: Task<()>,
}

impl Drop for StatusItem {
    fn drop(&mut self) {
        NSStatusBar::systemStatusBar().removeStatusItem(&self.item);
        CHOSEN.with(|chosen| chosen.borrow_mut().take());
    }
}

impl StatusItem {
    /// Puts the item in the menu bar. `None` off the main thread.
    pub fn install(cx: &mut App) -> Option<StatusItem> {
        let mtm = MainThreadMarker::new()?;
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        if let Some(button) = item.button(mtm) {
            button.setImage(Some(&mark(mtm)));
            button.setToolTip(Some(&NSString::from_str("kōan")));
        }
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        item.setMenu(Some(&menu));
        // SAFETY: `init` on a fresh instance of a class with no fields.
        let target: Retained<Target> = unsafe { msg_send![Target::alloc(mtm), init] };

        let (chosen, mut choices) = unbounded();
        CHOSEN.with(|slot| *slot.borrow_mut() = Some(chosen));
        let answers = cx.spawn(async move |cx| {
            while let Some(command) = choices.next().await {
                cx.update(|cx| match command {
                    Command::PlayPause => cx.dispatch_action(&TogglePlayPause),
                    Command::Next => cx.dispatch_action(&Next),
                    Command::Open => super::to_front(cx),
                    Command::Quit => cx.quit(),
                });
            }
        });
        let playback = cx.global::<Mirror>().playback.clone();
        let observing = cx.observe(&playback, |playback, cx| {
            let shown = shown(playback.read(cx));
            if cx.has_global::<super::residency::Residency>()
                && let Some(item) = cx.global_mut::<super::residency::Residency>().status_mut()
            {
                item.show(shown);
            }
        });
        let mut status = StatusItem {
            item,
            menu,
            target,
            shown: None,
            _playback: observing,
            _answers: answers,
        };
        status.show(shown(playback.read(cx)));
        Some(status)
    }

    /// Builds the menu for `shown`, when it differs from what it shows.
    fn show(&mut self, shown: Shown) {
        if self.shown.as_ref() == Some(&shown) {
            return;
        }
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        self.menu.removeAllItems();
        let label = |text: &str| {
            let item = self.entry(mtm, text, None);
            item.setEnabled(false);
            item
        };
        match &shown.title {
            Some(title) => {
                self.menu.addItem(&label(title));
                if let Some(artist) = &shown.artist {
                    self.menu.addItem(&label(artist));
                }
            }
            None => self.menu.addItem(&label("Nothing playing")),
        }
        self.menu.addItem(&NSMenuItem::separatorItem(mtm));
        let playing = shown.title.is_some();
        let play = self.entry(
            mtm,
            if shown.playing { "Pause" } else { "Play" },
            Some(sel!(playPause:)),
        );
        play.setEnabled(playing);
        self.menu.addItem(&play);
        let next = self.entry(mtm, "Next", Some(sel!(next:)));
        next.setEnabled(playing);
        self.menu.addItem(&next);
        self.menu.addItem(&NSMenuItem::separatorItem(mtm));
        self.menu
            .addItem(&self.entry(mtm, "Open kōan", Some(sel!(open:))));
        self.menu
            .addItem(&self.entry(mtm, "Quit kōan", Some(sel!(quit:))));
        self.shown = Some(shown);
    }

    fn entry(
        &self,
        mtm: MainThreadMarker,
        title: &str,
        action: Option<objc2::runtime::Sel>,
    ) -> Retained<NSMenuItem> {
        // SAFETY: a menu item with a title, an action this item's target
        // answers, and no key equivalent; the target outlives the menu.
        unsafe {
            let item = NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(title),
                action,
                &NSString::from_str(""),
            );
            if action.is_some() {
                item.setTarget(Some(&self.target));
            }
            item
        }
    }
}

fn shown(playback: &Playback) -> Shown {
    let now = &playback.0;
    let entry = now.entry.as_ref();
    Shown {
        title: entry.map(|e| e.title.clone()),
        artist: entry.map(|e| e.artist.clone()).filter(|a| !a.is_empty()),
        playing: now.state == koan_ffi::PlayState::Playing,
    }
}

/// kōan's ensō as a template image, which the menu bar tints for its
/// appearance: its mark among other apps', whatever the look. The app's icon
/// where the drawing cannot be read.
fn mark(mtm: MainThreadMarker) -> Retained<NSImage> {
    let svg = crate::components::enso_svg();
    let data = NSData::with_bytes(svg.as_bytes());
    if let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) {
        image.setSize(NSSize::new(16.0, 16.0));
        image.setTemplate(true);
        return image;
    }
    let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
    let icon = app.applicationIconImage().unwrap_or_default();
    icon.setSize(NSSize::new(18.0, 18.0));
    icon
}
