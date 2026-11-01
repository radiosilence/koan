# Vendored crates

Copies of published crates carried with patches until the patches are taken upstream. Each is wired in through `[patch.crates-io]` in the workspace `Cargo.toml` and excluded from the workspace, so it builds under its own settings and `cargo fmt --all` leaves it alone. Every change to a copy is marked with a `KOAN:` comment; `grep -rn KOAN: vendor/` lists them.

## What is carried

| Crate | Version | Patches |
|---|---|---|
| `gpui-pre` | 0.3.10 | Shader quads (#1641): `src/shader.rs`, `scene.rs`, `gpui.rs`, `window.rs` |
| `gpui-pre-apple` | 0.3.10 | Shader quads drawn by Metal (#1641): `metal_renderer.rs` |
| `gpui-pre-wgpu` | 0.3.10 | Shader quads skipped until the wgpu renderer draws them (#1641): `wgpu_renderer.rs` |
| `gpui-pre-macos` | 0.3.10 | Idle frames (#1609): `window.rs`, `display_link.rs` |
| `gpui-pre-linux` | 0.3.10 | Idle frames on X11 (#1609): `linux/x11/client.rs`, `window.rs` |

`gpui-pre`'s examples and benches are left out of the copy, and their sections out of its `Cargo.toml`.

## gpui-pre, gpui-pre-apple, gpui-pre-wgpu (0.3.10): shader quads

GPUI draws a fixed set of primitives, and something that changes every frame, such as the playing bars, costs a GPUI frame each time it moves: render, layout, prepaint, paint and a present. The patch adds one more primitive, a quad drawn by a fragment program the app supplies (`ShaderProgram`, MSL and WGSL source of one `shade` function), with sixteen uniforms the app writes from any thread (`ShaderUniforms`). The renderer reads the uniforms when it presents a frame, not when the frame is drawn, and while any shader on screen is live, GPUI's frame callback presents the last frame again on each refresh without drawing a new one, and keeps asking for refreshes; when none is, the window parks as before. `Window::paint_shader` paints one. Metal compiles each program the first time it is drawn, between a prelude declaring the instance and the vertex and fragment functions that call `shade`, and clips to the content mask in the fragment function. The wgpu renderer does not draw shaders yet, and Windows' DirectX renderer is not patched (it would not build against the new batch kind).

## gpui-pre-macos, gpui-pre-linux (0.3.10)

GPUI's macOS and X11 backends ask for a frame on every display refresh for as long as a window is visible, and GPUI returns early from each one nothing invalidated: a window at rest wakes the main thread 60 to 120 times a second (#1609). GPUI's core already tells a backend when a frame is wanted, through `PlatformWindow::frame_waker` (when the window first becomes dirty) and `schedule_frame` (when a frame leaves work for the next, or a next-frame callback is queued). The Wayland backend implements both and asks the compositor for a frame only then; these patches do the same for the other two.

- **macOS** (`gpui-pre-macos/src/window.rs`, `display_link.rs`): the window's display link parks after three frames no one asked for. `frame_waker` and `schedule_frame` only signal a main-queue dispatch source, since GPUI can call them while the window's state is locked; its handler restarts a parked link. Visibility still starts and stops the link as before. After herdr-app's patch for gpui-pre-macos 0.3.8.
- **X11** (`gpui-pre-linux/src/linux/x11/client.rs`, `window.rs`): the refresh timer drops itself after three refreshes no one asked for and the window's refresh state becomes `Parked`. A window's waker sets a flag of its own and rings a calloop ping, so it never borrows the client's state; the ping's handler restarts the timers of parked, visible windows that asked for a frame.

To move to a new GPUI release: copy the release's crates from `~/.cargo/registry/src/`, reapply every `KOAN:` hunk, and check that the release has not taken the change upstream already.
