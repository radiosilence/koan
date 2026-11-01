#![cfg(any(target_os = "linux", target_os = "freebsd"))]
// KOAN: a vendored copy; upstream lints it under its own settings.
#![allow(warnings, clippy::all)]
mod linux;

pub use linux::{current_platform, linux_platform};
