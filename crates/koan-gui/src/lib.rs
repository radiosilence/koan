//! The kōan GUI app, drawn with GPUI.

/// A key with the platform's command modifier: ⌘ on macOS, Ctrl elsewhere.
/// GPUI's own `secondary-` needs the OS when the binding is parsed, which
/// `KeyBinding::new` does not pass.
macro_rules! secondary {
    ($key:literal) => {
        if cfg!(target_os = "macos") {
            concat!("cmd-", $key)
        } else {
            concat!("ctrl-", $key)
        }
    };
}

pub mod app;
pub mod components;
pub mod covers;
pub mod engine;
#[cfg(feature = "profiler")]
pub mod measure;
pub mod mirror;
pub mod navigator;
pub mod paint;
pub mod queue;
pub mod root;
pub mod session;
pub mod sheet;
pub mod shell;
pub mod sidebar;
pub mod theme;
pub mod time;
pub mod transport;
pub mod window_state;
