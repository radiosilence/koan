# Plan 05: GUI-Grade Mouse Support

## Summary

koan already has solid mouse foundations: click-to-seek, double-click-to-play, drag-to-reorder, scrollbar dragging, multi-select with modifiers, and scroll wheel. The gap is *feel* -- hover feedback, right-click menus, smooth interactions, and visual polish that makes a TUI feel like a proper GUI.

crossterm 0.28 (current) already enables `?1003h` (any-event tracking) + `?1006h` (SGR extended coordinates) via `EnableMouseCapture`. This means **mouse move events are already arriving** -- koan just drops them in the `_ => {}` catch-all of `handle_mouse`. Every modern macOS terminal supports these modes. The plumbing is there; we just need to use it.

The hard constraint: terminals render on a character grid, so hover "effects" are style changes applied during the next render frame, not continuous GPU compositing. The 50ms tick rate (20 FPS) is the bottleneck for perceived responsiveness, not the mouse event rate.

---

## Current State Analysis

### What Works

| Feature | Implementation | Location |
|---------|---------------|----------|
| Double-click to play | Timer-based, 400ms window | `app.rs:1020-1031` |
| Click-to-seek | `TransportBar::seek_from_click` | `transport.rs:49-61` |
| Scroll wheel | Queue, library, picker (3-line steps) | `app.rs:1112-1152` |
| Drag-to-reorder | Live reorder via `PlayerCommand` | `app.rs:1059-1105` |
| Scrollbar click+drag | `scroll_to_scrollbar_y` | `app.rs:1578-1591` |
| Multi-select | Shift/Ctrl range, Alt toggle | `app.rs:1012-1044` |
| Library click | Click to select, arrow-area expand | `app.rs:904-950` |
| Click-outside-to-close | TrackInfo, Organize, ContextMenu, Picker | `app.rs:766-898` |
| Cover art click-to-zoom | Clicks now-playing art area | `app.rs:954-959` |
| Drop indicator | `last_mouse_row` tracks hover position | `app.rs:758-761` |

### What's Missing

- **No hover effects** -- items don't highlight on mouseover
- **No right-click** -- `MouseButton::Right` is never matched
- **No `MouseEventKind::Moved`** -- falls through to `_ => {}`
- **No cursor shape changes** -- always default pointer
- **No panel resize** -- library/queue split is hardcoded 40/60
- **No tooltip-like info** -- no hover-triggered metadata display
- **No smooth/pixel scrolling** -- 3-line jumps only
- **Library scrollbar** -- library pane has no scrollbar at all
- **Context menu always centered** -- not positioned at click point

### Architecture: LayoutRects

The existing `LayoutRects` struct (app.rs:81-94) caches render-time geometry for hit-testing. This is the right pattern. Every widget area is stored after render, then mouse events check `is_in_rect()`. Extending this for hover just means tracking which rect the cursor is currently over.

---

## Terminal Capability Matrix

### Mouse Protocol Support

crossterm's `EnableMouseCapture` sends: `?1000h` (normal) + `?1002h` (button-event) + `?1003h` (any-event) + `?1015h` (RXVT) + `?1006h` (SGR).

| Terminal | SGR 1006 | Any-event 1003 | Move events | Scroll wheel | Modifier keys on mouse | Notes |
|----------|----------|----------------|-------------|-------------- |----------------------|-------|
| **iTerm2** | Yes | Yes | Yes | Yes | Shift, Ctrl, Alt | Best macOS support |
| **Terminal.app** | Yes | Yes | Yes | Yes | Limited | Shift-click unreliable |
| **Alacritty** | Yes | Yes | Yes | Yes | All | Minimal, fast |
| **kitty** | Yes | Yes | Yes | Yes | All | Also has kitty protocol |
| **WezTerm** | Yes | Yes | Yes | Yes | All | Full SGR support |
| **Ghostty** | Yes | Yes | Yes | Yes | All | Newer, solid support |
| **foot** | Yes | Yes | Yes | Yes | All | Wayland only |

**Bottom line**: Every terminal koan users would realistically use supports full mouse tracking. No compatibility concerns.

### OSC 22 Mouse Pointer Shape Support

The kitty pointer shape protocol (OSC 22) allows apps to change the mouse cursor (pointer, grab, crosshair, etc.).

| Terminal | OSC 22 | Working shapes | Notes |
|----------|--------|---------------|-------|
| **kitty** | Yes | All 30 CSS shapes | Originator of the spec |
| **Ghostty** | Partial | default, pointer, text | Other shapes broken on macOS |
| **WezTerm** | Partial | pointer, text, resize variants | Limited set |
| **foot** | Yes | Full | Wayland, solid support |
| **Alacritty** | No (PR pending) | None | Draft PR exists |
| **iTerm2** | Buggy | None (parsed but no effect) | Known bug |
| **Terminal.app** | No | None | No support |

**Verdict**: OSC 22 is nice-to-have but unreliable outside kitty/foot. Implement it behind a feature-detect (query + response), fall back gracefully. Never depend on it for UX -- it's polish, not functionality.

### Pixel Mouse Coordinates

Some terminals (kitty, Ghostty) support reporting mouse coordinates in pixels rather than cells. This enables sub-cell-resolution scrolling. crossterm doesn't expose this natively -- would need raw escape sequence parsing. **Not worth it** for the complexity; cell-based coordinates are sufficient with proper interpolation.

---

## Interaction Design Catalog

### 1. Hover Highlighting

**What**: Items under the mouse cursor get a subtle visual distinction from their default state.

**Implementation**:

```
// New state in App
pub hover: HoverState,

#[derive(Default)]
pub struct HoverState {
    pub column: u16,
    pub row: u16,
    pub zone: HoverZone,
}

#[derive(Default, PartialEq)]
pub enum HoverZone {
    #[default]
    None,
    QueueItem(usize),       // index into visible queue
    LibraryItem(usize),     // index into library nodes
    SeekBar,
    ScrollbarQueue,
    ScrollbarLibrary,
    TransportArt,
    TransportText,
    PanelDivider,
    PickerItem(usize),
    ContextMenuItem(usize),
}
```

**In `handle_mouse`**: On `MouseEventKind::Moved`, compute `HoverZone` from coordinates using the same `is_in_rect` + `queue_index_at_y` logic already used for clicks. Store it. That's it -- no commands, no side effects.

**In render**: Pass `hover.zone` to widgets. Each widget checks if its item index matches and applies a hover style (e.g., dim underline or subtle bg color change).

**Theme additions**:
```rust
pub track_hover: Style,      // subtle: dim underline or very slight bg
pub library_hover: Style,
pub seekbar_hover: Style,    // maybe brighter bar color
```

**Performance**: `Moved` events fire on every cell the cursor crosses. At a 50ms tick, we only process one per frame anyway (the last one polled). The cost is one `HoverZone` computation per frame when the mouse moves -- trivially cheap. No re-render is triggered by hover alone; it piggybacks on the existing tick-driven render cycle.

**Feel details**:
- Queue: hover row gets a very subtle background tint (not as strong as cursor/selection)
- Library: same treatment
- Seek bar: filled portion brightens slightly, or a position marker appears
- Scrollbar: thumb gets a brighter style on hover
- No hover delay -- instant feedback. Delay feels sluggish in a TUI.

### 2. Right-Click Context Menus

**What**: Right-click opens a context menu *at the click position*, with actions relevant to what was clicked.

**Implementation**:

The existing `ContextMenuState` and `ContextMenuOverlay` infrastructure is there but currently only triggered by keyboard (`m` key) and always centered. Changes needed:

1. **Match `MouseButton::Right`** in `handle_mouse`:
   ```rust
   MouseEventKind::Down(MouseButton::Right) => {
       // Determine what was right-clicked
       if in_queue_area {
           // Select the clicked item if not already selected
           // Open context menu at (event.column, event.row)
       } else if in_library_area {
           // Library-specific context menu
       } else if in_transport_area {
           // Transport context menu (copy track info, etc.)
       }
   }
   ```

2. **Position-aware context menu**: Store click position in `ContextMenuState`, use it in `context_menu_rect` instead of centering. Clamp to terminal bounds so it doesn't overflow.

3. **Richer actions per context**:
   - Queue item: Play, Remove, Move to top, Track info, Organize, Copy path
   - Library item: Enqueue, Enqueue & play, Replace queue
   - Transport: Copy "Artist - Title", Open file location
   - Scrollbar: (no menu)

4. **Mouse-aware menu navigation**: Track hover within the context menu area, highlight the hovered action row (already partially done for keyboard cursor).

### 3. Drag and Drop State Machine

**Current state**: Drag reorder works for single and multi-selected items. The `DragState` struct tracks `from_index`, `current_y`, and `multi`.

**Improvements needed**:

```
enum DragOperation {
    /// Reordering within the queue (existing behavior).
    Reorder { from_index: usize, multi: bool },
    /// Dragging the scrollbar thumb.
    ScrollbarDrag,
    /// Resizing the library/queue panel divider.
    PanelResize { start_x: u16, original_ratio: f32 },
    /// Dragging the seek bar position (scrubbing).
    SeekScrub,
}
```

**Drag threshold**: Currently, a click immediately starts a drag. This causes accidental micro-drags. Add a 3-pixel (cell) dead zone:
```rust
pub struct DragState {
    pub operation: DragOperation,
    pub start_col: u16,
    pub start_row: u16,
    pub current_col: u16,
    pub current_row: u16,
    pub committed: bool, // true once threshold exceeded
}
```
Only begin the actual reorder/resize once `|current - start| > threshold`. Until then, treat it as a pending click.

**Seek scrubbing**: Click-and-hold on the seek bar should continuously update position as the mouse moves horizontally. Currently only single-click works. Add `DragOperation::SeekScrub` handling in `MouseEventKind::Drag`.

### 4. Scrollbars

**Current state**: Queue has a custom scrollbar (queue.rs:196-218). It's a basic thumb-on-track. Click and drag work (app.rs scrollbar_dragging). Library has no scrollbar.

**Improvements**:

Option A: **Use `tui-scrollbar` crate** -- provides fractional 1/8th-cell thumb rendering, proper mouse event handling via `handle_mouse_event` -> `ScrollCommand`, and `ScrollBarInteraction` for drag persistence. Supports crossterm 0.28/0.29. This would replace the hand-rolled scrollbar.

Option B: **Enhance the existing scrollbar** -- add hover highlight (brighter thumb on hover), smooth thumb sizing, and replicate the same pattern for library.

**Recommendation**: Option B. The existing scrollbar is simple and works. Adding `tui-scrollbar` as a dependency for marginal visual improvement isn't worth it. Instead:
- Extract the scrollbar rendering into a reusable helper function
- Add it to the library pane
- Add hover highlighting (check `HoverZone::ScrollbarQueue` / `ScrollbarLibrary`)
- Keep the existing click-drag behavior

### 5. Panel Resize by Dragging Divider

**What**: Drag the border between library and queue panes to resize them.

**Implementation**:

1. **Detect divider**: In library mode, the divider is at `library_area.x + library_area.width`. Store this as `layout.panel_divider_x`.

2. **Hit test**: If `MouseEventKind::Down(Left)` on `panel_divider_x` (+/- 1 col tolerance), start `DragOperation::PanelResize`.

3. **During drag**: Compute new ratio from `event.column` relative to content area width. Clamp to 20%-80% range.

4. **State**: Add `pub library_ratio: f32` to `App` (default 0.4). Use in `ui.rs` instead of `Constraint::Percentage(40)`.

5. **Visual feedback**: When hovering over the divider, change cursor shape to `ew-resize` (OSC 22 where supported). Show a slightly different border character.

6. **Persistence**: Optionally save `library_ratio` to config.local.toml so it survives restarts.

### 6. Click-to-Select with Modifiers

**Current state**: Already implemented. Shift/Ctrl for range select, Alt for toggle select.

**Improvements**:
- **Shift-click in Normal mode**: Currently range select only works in QueueEdit mode conceptually, but the code applies in any mode. Document this behavior.
- **Rubber-band selection**: Drag in empty queue area to select a range. Currently drag always starts a reorder. Only start reorder if the click hits an item; if it hits empty space, start a rubber-band.

### 7. Tooltip-like Hover Info

**What**: Hovering over a truncated track title shows the full text. Hovering over a codec badge shows detailed format info.

**Implementation considerations**:
- Terminals can't do floating tooltips like a GUI. The best approximation is a status-bar line or a brief overlay.
- **Approach**: Use the hint bar (bottom row) as a context-sensitive info line. When `HoverZone::QueueItem(idx)` is active, show the full `"Artist - Title (Album, Year) [codec detail]"` in the hint bar. When not hovering, show the key hints as normal.
- This is cheap -- just a conditional in `HintBar::new()` that checks hover state.
- **Alternative**: A small popup near the cursor (like a tooltip). This is harder -- needs an overlay render pass, position clamping, and a hover delay to avoid flickering. Defer to Phase 3.

### 8. Double-Click Expand/Collapse in Library

**Current state**: Double-click in library enqueues. Single click on the arrow area (<4 cols) toggles expand/collapse.

**Change**: Double-click on an Artist or Album node should toggle expand/collapse (matching typical tree-view behavior). Double-click on a Track node enqueues it. This is more intuitive than the current "click arrow to expand, double-click text to enqueue" split.

**Implementation**: In the library click handler, check `LibraryNode` type at cursor. If Artist/Album: double-click toggles expand. If Track: double-click enqueues.

### 9. Smooth Scrolling

**Current state**: Scroll wheel moves 3 lines per event. This feels jerky.

**Options**:

A. **Reduce step size**: 1 line per scroll event. Simple, immediate improvement. Most mice send multiple scroll events per physical notch anyway.

B. **Animated scrolling**: Scroll events set a target offset, render interpolates toward it over 2-3 frames. This gives a smooth glide effect.
   ```rust
   pub scroll_target: usize,  // where we want to be
   pub scroll_offset: usize,  // where we are (rendered)
   // Each frame: scroll_offset += (scroll_target - scroll_offset).signum()
   ```
   At 20 FPS, a 3-line scroll would animate over 150ms (3 frames) -- perceptible but not sluggish.

C. **Pixel-based scrolling**: Requires kitty/Ghostty pixel mouse mode. Not worth the complexity.

**Recommendation**: Start with A (1-line steps), then add B (animated) in Phase 2. The 50ms frame time makes animation look decent but not silky -- if we bump to 30ms (33 FPS) it'll feel noticeably better. Consider making the tick rate configurable or adaptive (faster when mouse is active).

### 10. Mouse Cursor Shape Changes (OSC 22)

**What**: Change the mouse pointer to indicate affordance -- `pointer` over clickable items, `grab`/`grabbing` during drag, `ew-resize` over panel divider, `text` over text fields.

**Implementation**:

```rust
fn emit_pointer_shape(shape: &str) {
    // OSC 22 ; > shape ST
    print!("\x1b]22;>{}\x1b\\", shape);
}
```

**When to change**:
- `HoverZone::QueueItem(_)` -> `pointer`
- `HoverZone::SeekBar` -> `pointer`
- `HoverZone::PanelDivider` -> `ew-resize`
- `HoverZone::ScrollbarQueue` -> `default`
- During drag reorder -> `grabbing`
- During panel resize -> `ew-resize`
- Default -> `default`

**Terminal detection**: Query with `OSC 22 ; ? ST` and check for a response. If no response within ~50ms, disable pointer shape changes for the session. Store as `pub supports_osc22: bool` in App.

**Graceful degradation**: Terminals that don't support OSC 22 silently ignore the escape sequence (or display garbage briefly). The query-first approach avoids this. On `Terminal.app` and `iTerm2`, this feature will be disabled.

---

## Architecture Changes Needed

### 1. HoverState (New)

Add `HoverState` to `App`. Updated every frame from `MouseEventKind::Moved` events.

### 2. DragState Refactor

Replace the current flat `DragState` with the richer `DragOperation` enum to support multiple drag types (reorder, scrollbar, panel resize, seek scrub).

### 3. Theme Expansion

Add hover styles to `Theme`. These should be subtler than selection/cursor styles -- think "slight warmth" not "highlight".

### 4. LayoutRects Expansion

Add:
```rust
pub panel_divider_x: Option<u16>,  // library/queue border
pub library_scrollbar_area: Rect,   // for library scrollbar
pub hint_bar_area: Rect,            // for hover-info display
```

### 5. Widget API Changes

Widgets that need hover awareness need the hover index passed in:
- `QueueView::new()` gets `hover_index: Option<usize>`
- `LibraryView::new()` gets `hover_index: Option<usize>`
- `TransportBar` could highlight the seek position on hover

This is purely additive -- existing callers just pass `None` until hover is wired up.

### 6. Event Loop Consideration

Currently the event loop is: poll(50ms) -> handle -> render. Mouse move events between frames are lost. This is fine -- we only care about the *latest* mouse position, not every intermediate one. The poll reads one event at a time, so rapid mouse movement means we process one move event per frame. This is correct behavior.

If we want higher responsiveness during drag operations, we could drain all pending events before rendering:
```rust
loop {
    let event = poll(Duration::from_millis(50))?;
    // Drain any buffered events
    while crossterm::event::poll(Duration::ZERO)? {
        match crossterm::event::read()? {
            CtEvent::Mouse(m) => last_mouse = Some(m),
            CtEvent::Key(k) => { handle_key(k); }
            _ => {}
        }
    }
    if let Some(m) = last_mouse { handle_mouse(m); }
    render();
}
```
This ensures we always render with the latest mouse position, even if multiple move events queued up during a slow render. **Recommended for Phase 1**.

---

## Performance Considerations

### Mouse Move Event Rate

With `?1003h` (any-event tracking), the terminal sends a mouse event for every cell the cursor crosses. On a 200-column terminal with a fast mouse swipe, that's potentially hundreds of events per second. Since koan polls at 50ms intervals, at most ~20 events/sec are processed (one per frame). The rest buffer in the terminal's output pipe.

**Risk**: Event buffer grows during slow renders. The drain-all-events approach above handles this.

**CPU impact**: Computing `HoverZone` from coordinates is O(1) -- just a few rect comparisons. Building display lines for `queue_index_at_y` is O(n) where n = queue length, but this is already called on every click. For hover, we'd call it on every frame where the mouse moved. With a 1000-track queue this is still sub-microsecond.

### Render Impact

Adding hover styles means the buffer differs more between frames (hover style changes as mouse moves). ratatui's diff-based rendering handles this efficiently -- only changed cells are written to the terminal.

### Memory

`HoverState` is 5 bytes. `DragState` refactor adds maybe 16 bytes. Negligible.

### Adaptive Frame Rate

Consider bumping from 50ms to 33ms (30 FPS) when mouse is actively moving, dropping back to 50ms after 500ms of no mouse activity. This makes hover feel snappier without wasting CPU when idle.

```rust
let tick = if app.hover.recently_active() {
    Duration::from_millis(33)
} else {
    Duration::from_millis(50)
};
```

---

## Phased Implementation Plan

### Phase 1: Hover Foundation + Quick Wins

**Effort**: ~2-3 sessions

1. **Add `HoverState` to App** -- struct, zone enum, default
2. **Handle `MouseEventKind::Moved`** -- compute zone, store it
3. **Event drain loop** -- process all buffered events before render
4. **Hover styles in Theme** -- subtle underline or faint bg for queue/library items
5. **Pass hover to QueueView** -- highlight hovered row
6. **Pass hover to LibraryView** -- highlight hovered row
7. **Seek bar hover** -- show position indicator on hover
8. **Scrollbar hover** -- brighter thumb when hovered
9. **Scroll step reduction** -- 1 line per scroll event instead of 3
10. **Right-click on queue item** -- open context menu at click position

**Deliverable**: Mouse move tracking works, items highlight on hover, right-click opens a positioned context menu, scrolling feels better.

### Phase 2: Rich Interactions

**Effort**: ~2-3 sessions

1. **DragState refactor** -- `DragOperation` enum, drag threshold (3-cell dead zone)
2. **Seek scrubbing** -- click-hold-drag on seek bar for continuous seeking
3. **Library scrollbar** -- extract scrollbar helper, add to library pane
4. **Panel divider resize** -- detect divider, drag to resize, persist ratio
5. **Context-sensitive hint bar** -- show full track info on hover in hint bar
6. **Double-click library tree** -- expand/collapse for Artist/Album, enqueue for Track
7. **Animated scrolling** -- scroll target + interpolation over 2-3 frames
8. **Adaptive frame rate** -- 33ms when mouse active, 50ms when idle

**Deliverable**: Dragging feels professional (threshold prevents accidental), seek scrubbing works, library has a scrollbar, panel resize works.

### Phase 3: Polish

**Effort**: ~1-2 sessions

1. **OSC 22 pointer shapes** -- detect support, change cursor based on hover zone
2. **Richer context menus** -- per-zone actions (library, transport, empty area)
3. **Hover tooltips** -- small overlay popup for truncated text (with 300ms hover delay)
4. **ScrollLeft/ScrollRight** -- horizontal scroll in picker results or long text
5. **Rubber-band selection** -- drag in empty queue area to select a range
6. **Middle-click paste** -- paste paths from clipboard (X11-style, macOS pbpaste fallback)

**Deliverable**: The TUI feels like it was designed mouse-first. Every interactive element responds to hover, every action is reachable by mouse alone.

---

## What "Feels Like a GUI" Actually Means

The difference between "TUI with mouse support" and "feels like a GUI" is in the micro-interactions:

1. **Everything reacts** -- nothing feels dead. Hover over it, something changes. Even if it's just a subtle underline.

2. **No accidental actions** -- drag threshold prevents micro-drags. Double-click timing is forgiving (400ms is good).

3. **Visual continuity** -- the seek bar tracks the mouse during scrub. The panel divider follows the cursor during resize. The scrollbar thumb moves with the drag, not jumping.

4. **Context menus appear where you click** -- not centered on screen. They feel attached to the thing you right-clicked.

5. **Cursor tells you what will happen** -- pointer over clickable, resize handles over dividers, grab during drag. This is the weakest link due to terminal limitations, but OSC 22 covers kitty/foot users.

6. **Scroll feels natural** -- 1-line steps with gentle animation. Not 3-line jumps.

7. **Mouse and keyboard are peers** -- every mouse action has a keyboard equivalent, and vice versa. Neither is second-class.

koan already nails #7. Phases 1-3 add the rest.
