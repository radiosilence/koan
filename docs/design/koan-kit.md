# koan-kit: the design system

How the desktop app stays consistent: one design system, the one the Mac app already has (`docs/design/koan-theme.md` and the roles in `apps/macos/Sources/Koan/Support/KoanTheme.swift`), rebuilt for GPUI role for role. Pages declare what they show; where it sits, how big it is and what colour it takes are decided below them.

## Why

The Mac app got its consistency from SwiftUI's containers. `Form`, `Section`, `List`, `.toolbar` and the environment decide spacing, row metrics and type, and `KoanTheme` restyles those containers once. A page written against them cannot drift, because the spacing is not its to choose. GPUI has no such containers: every `div()` takes any padding, size and colour, so a page built by hand chooses all three and drifts. The foundation puts the containers back.

## Crates

```
gpui-kit  ->  koan-kit  ->  koan-gui
```

**koan-kit** (`crates/koan-kit`) is the design system: the theme (tokens, type roles, Geist Mono, the icon set, the accent, the wash), every control and container below, the placeholder and spinner, and the role map in this document. It knows nothing of the engine or kōan's data (no `koan-ffi`, no `koan-core`), so it builds and is tested alone, and `cargo run -p koan-kit` opens its storybook: every component and container on a page of its own, in each state, with the appearance, the accent (a sample sleeve among them, for its tone mapping and the wash), the text size, Reduce Motion and Show icons to switch. `examples/storybook.rs` draws each page offscreen, dark and light, to a PNG.

**koan-gui** builds pages from koan-kit only. It does not depend on `gpui-kit`: koan-kit re-exports what pages need to be views (entities, contexts, windows, tasks, actions, key bindings, `div`, `list`, `uniform_list`, `img`, events, drag and drop, the `Styled` and `ParentElement` traits) and leaves out what would let a page choose a look (`px`, `rems`, `rgb`, `rgba`, `hsla`, `Hsla` construction, `font`, fonts and text styles, gpui-component's widgets). Going around the system is then a compile error rather than a lint. What a type cannot stop, a method on a re-exported trait, the lint below catches.

## Beyond kōan

koan-kit is written to be used by other projects (sbr, say) and published to crates.io later, so it stays project-agnostic: no koan-gui types, none of the app's wording, routes or data. Words a component draws come from its caller; a cover is `CoverArt`, never a library record. The kōan look is its default; an app supplies its own accent (the theme's `AccentFeed` takes any colour) and, in time, its own icon set through the asset source. It has a standalone storybook, its own docs, and tests of its own.

One thing does not travel: the GPUI patch that stops an idle window waking at every display refresh (#1614) is vendored in this workspace through `[patch.crates-io]`, which applies to this workspace alone. Another project using koan-kit gets GPUI as published until the patch is taken upstream.

## Built without the Mac's glitches

The Swift app fixed its glitches one at a time: a seek bar that snapped back, menus that reopened when their button was clicked to close them, hover jank, scroll jumps, layout shifts on load, covers jumping while a window resized. Each koan-kit component is built from its Swift counterpart's history (`git log -p` on the view, and the merged PRs that fixed it), so it has those fixes by construction, with a test where one can be written, and its docs list them. Two so far:

- **Seeking.** A seek lands at once and stale playhead anchors wait until the engine reaches it, or two seconds pass (`PlayerModel.swift`'s pending seek). Shared by every app in koan-ffi's published state, and tested there.
- **Menus and popovers toggle.** A trigger whose menu is open closes it, including when the click outside that dismisses the menu and the trigger's own click arrive in one event. Part of the anchor every menu and popover trigger uses.

## Layers

| Layer | Where | Owns |
|---|---|---|
| Tokens | `koan-kit/theme/tokens.rs` | The palette, `TypeRole` (sizes, weights, line heights), `Space`, `Scale` (text size), `Motion`, `HAIRLINE`, `HIT` |
| Environment | `koan-kit/theme`: `Look::of(window, cx)` | The tokens resolved for this window: appearance, text size, icons on or off, the record's accent, and the system's contrast and transparency settings |
| Roles | `koan-kit/theme` | `text_role`, `Tone`, `label()` (app words lowercase), `KoanIcon` and the glyph set |
| Controls | `koan-kit/components` | One interactive thing each: buttons, toggles, fields, sliders, steppers, pickers, badges, spinner, placeholder |
| Containers | `koan-kit/components` | Layout: page, page header, toolbar, form, table and rows, menu, empty state, sheet, scroll area |
| Pages | koan-gui: `library/`, `search/`, `queue/`, `settings/`, `dsp/`, `shell.rs`, `sidebar.rs`, `transport.rs` | Content only, composed from containers and controls, and the engine's data |

Each layer reads only the ones above it. A page never reaches into tokens for a number.

## How theme values reach components

As SwiftUI's environment reaches a view: from one place, inherited, never passed by hand. Every component calls `Look::of(window, cx)` in its `render`. The window's appearance, the text size, "show icons" and the accent all live there. A component takes semantic parameters (a `Tone`, a `TypeRole`, a `RowKind`, a `ButtonKind`) and never a colour, a size in points or a font. A cached view that draws the accent calls `theme::observe_accent` so an accent change reaches it. Nothing in a page computes a size from the scale; the containers do (`RowMetrics`, `Look::space`, `Look::spaced`).

### The system's settings

The theme follows what the system asks for, read where the platform says it and again on its change notifications, never polled:

- **Reduce Motion** (macOS): every transition, fade and ease lands at once, the wash and the spinner stand still, and an unknown amount of progress is a still arc. Low Power Mode does the same for decorative motion.
- **Increase Contrast** (macOS) and high contrast (the desktop portal's `contrast`, which GNOME sets): `Look` takes the contrast palettes (`Palette::DARK_CONTRAST`, `LIGHT_CONTRAST`), where every text token reaches 7:1 on `bg` and `surface` and `rule` 3:1; the accent is held to 7:1 on both grounds (`Shade::held`); rules are twice as thick (`Look::hairline`); and a picked row is outlined as well as filled. `every_text_token_reads_on_every_ground` checks the palettes.
- **Reduce Transparency** (macOS): no wash, the window's ground in its place, and the veils behind sheets and the artwork viewer near opaque (`Look::veil`).
- **Text size** (GNOME's `text-scaling-factor`, through the portal): the app's own text size multiplies it, so 100 % is the system's size (`Scale::at`). macOS has no system-wide size, and the app's setting is the control there.

## What pages decide, and what they may not

Pages decide **what**: which content, in what order, which verbs a header or menu offers, which columns a table shows (from the set the table defines), what an empty page says, what a write asks before it runs.

Pages may not decide **where or how it looks**:

- no `px(…)`, `rgb(…)`, `hsla(…)`, font sizes or line heights;
- no margins, gaps or padding between sections, rows or header parts;
- no row heights, sleeve sizes or column widths of their own;
- no header, toolbar button, table row, menu panel, placeholder or loading state built from `div()`s.

When a page needs something no container offers, the container grows the capability (or a new one is added here) and every page gets it. That is the only way a new look enters the app.

## The pieces

API sketches; the module docs are authoritative once each lands.

### Page and PageHeader (Mac: `PageHeader`, the stage's page edges)

```rust
Page::new(
    PageHeader::new("history")                // app words: lowercase
        .detail("1,204 plays")                // control, muted
        .art(cover)                           // optional, top-aligned
        .lead(play_and_shuffle)               // optional, above the title
        .action(verb)                         // optional, under the detail
        .trailing(filter_and_sort),           // optional, far edge
)
.body(table)
```

`Page` owns the margins (the spec's 32 at the sides, 16 above, 12 to the body). `PageHeader` owns the Mac header's layout: 18 between art and text, 6 between title and detail, the title in `title`/`strong` on two lines at most. `PageHeader::data` keeps a title's case (a record, a playlist).

### Toolbar and ToolbarButton (Mac: `.toolbar` placements, `koanButton(.toolbar)`)

```rust
Toolbar::new()
    .leading(ToolbarButton::new("back", Icon::Back, "back").disabled(!can_go_back))
    .principal(title)
    .trailing(ToolbarButton::new("lyrics", Icon::Lyrics, "lyrics").selected(open))
```

`ToolbarMetrics` holds the Mac's unified toolbar, measured at 2x: 52 points tall with the traffic lights centred on it, 8 in from the edges, separate items 8 apart. A page's `ToolbarGroup` declares its groups (`spacer()` between them) and the toolbar spaces them: 2 points between the buttons of a group, a 16-point spacer between groups. `ToolbarButton` is the spec's toolbar button: the glyph in `ink` in a 36-point cell, `surface` under the pointer, `hover` pressed, the focus ring, 40 % disabled; the word in the cell with icons off.

`ToolbarSelection::new("3 selected").action(id, label, run)`: what a toolbar says while things are picked, the Mac's selection bar: the count in `muted`, then text buttons in the accent, the toolbar's group spacing between each.

`ToolbarMenu::new(id, button, content)`: a toolbar button that opens a menu panel under it (a page's ⋯, a sort), the panel built each time it opens, its rows given a `Dismiss` to call before they act.

### Drag and drop (Mac: the drag image, the list's drop indicator)

`DragChip::new(label).sleeve(art)` is what a drag shows under the pointer: the first record's sleeve, when the page knows one, and what is carried ("3 tracks", or the one thing's name). `.drop_highlight::<T>(look)` (`DropTarget`) lights a target in the accent while a drag of `T` is over it, and only then, so a drag over something that refuses it shows nothing. Between a list's rows, where a drop lands is the insertion line, `INSERTION` thick, in the accent.

### FilterField (Mac: `KoanSearchField`, the filter panel's field)

`FilterField::new(&input)`: no box, the search glyph and, 10 points after it, a lowercase prompt in `muted` that names what it narrows ("filter albums"); 167 points from the glyph to the end of the text at 100 %, as the Mac's field. The input takes the rest of the field's width: given only its content's, it is zero wide, shows no prompt and takes no click. A click anywhere on the field, a toolbar button's height, focuses it; in a narrow page column it gives up width down to 64 points before any toolbar button goes. The page owns the `InputState` and what typing does.

With text in it the field shows a clear mark at its trailing edge, `muted` and `ink` under the pointer; a press on it empties the field and keeps the keyboard there, and Escape does the same, going on to whatever Escape means around the field once it is empty. Emptying is an `InputEvent::Change` like typing. `.fill()` makes it as wide as what holds it, as the sidebar's search is. The input's types are `koan_kit::app::input`.

`SuggestionPanel` hangs under the nearest `relative` parent and paints over the window: `SuggestionHeading`s over groups of `SuggestionRow`s, each a sleeve (`art`) or a glyph (`icon`), a title and a line of `detail`, `lit` on `surface`. In a list things are picked from, `picked` puts the tick in the art's place and `figure` a length at the trailing edge.

### Table, TableRow and RowMetrics (Mac: `KoanTable`, `TableRow`, `TrackTableRow`, `RowMetrics`)

```rust
TableRow::new(id, RowKind::Art)
    .selected(picked)
    .lead(MARK, mark)            // a fixed cell, its content at its trailing edge
    .lead_exact(sleeve_side, sleeve)
    .title(title)                // flexes and truncates
    .line(credit)
    .figure(FORMAT, quality)     // fixed, right-aligned
    .figure(LENGTH, duration)
```

`RowMetrics` gives heights (line 24, text 34, art 40, times the text size, plus 4 above and below), the sleeve (32), the page margin and the gap between columns. A fixed cell's width is passed in points at 100 % and grows with the text size; each list names its widths as constants of its own (the queue's `MARK`), so nothing yet makes one figure the same width in every list. Rows have no hover fill (the user's call); a picked row is `selection` with its text in `on_selection`, and the current row is in the accent. A table is a `uniform_list` of rows inside `Page`'s body; `RowMetrics::page_margin` is the row's inset, so content lines up under the header.

### CommandBar (Arc's and Dia's command bars)

`CommandBar::new(id, &input).section(title, rows).lit(ix).key("⌘↩", "play next").empty(…).on_dismiss(…)`: a search over everything, centred over the dimmed window, the query in `title_small` across its top, results in headed groups of `SuggestionRow`s with the lit one kept in view, and along its foot what each key does. The owner holds the query and decides what is found and what each key does.

### Collection, CollectionSection, Pill (Mac: `MixedCollection`, `ArtistPill`)

```rust
Collection::new("shelf", &width)               // width: the page's CollectionWidth
    .header(PageHeader::new("favourites").detail(counts))
    .section(CollectionSection::pills(heading, pills))
    .section(CollectionSection::tiles(heading, grid, tiles))   // grid: width.grid(look)
    .section(CollectionSection::rows(heading, rows))
    .empty(EmptyState::new(icon, title))
```

Different kinds of thing answering one question, stacked in sections: pills wrapping, record tiles in as many columns as fit (`TileGrid`, the album grid's metrics), table rows the full width. The collection owns the margins, the section spacing, the pills' flow and the tiles' columns; it measures its width into the page's `CollectionWidth` and redraws the page when it changes, so the page builds tiles at the size they are laid out at. A section with nothing in it is left out. `SectionHeading::count` and `on_open` give a preview's heading its total and its "see all". `Pill::new(id, icon, name)` is a thing named as a chip, `ticking(picked)` while the page picks.

### Inspector, SyncedLines, Viewer, DetailGroup (Mac: the inspector, `LyricsPanel`, `ArtworkViewer`, `TrackInfoView`)

`Inspector::new("lyrics").trailing(source).body(…)` is a panel beside the page, `RowMetrics::inspector` wide, with a hairline down its leading edge and under its head; `clear(true)` leaves its ground to the wash. `SyncedLines::new(id, lines, active).on_line(…)` is timed text with the line at `active` larger and kept in view; `plain_text` is text that does not follow the song. `Viewer::new(id, art, title).detail(…)` is a picture as large as the window allows over everything, caption under it, closed by its owner on a click or Escape; `Viewer::side` is the size to decode it at. `DetailGroup::new(title).row(name, value)` lists named values densely under a section header, names in a column of their own.

### Form, FormSection, FormRow (Mac: `KoanForm`, `KoanSectionHeader`, `KoanLabeledContentStyle`)

```rust
Form::new("playback")
    .section(FormSection::titled("output").row(FormRow::new("device").control(picker)))
    .section(FormSection::new().row(toggle).footer("Why this matters."))
```

`Form` scrolls, padded 22, sections 16 apart. `FormSection` lays out its header (`fine`, `ink`, 16 above), its rows and its footer (`fine`, `muted`) 16 apart, as a SwiftUI `Section` in the Mac's form column does: the section's parts are the column's own children. `FormRow` is the Mac's two columns: the label in `body` in a 200-point column, then the value (`control`, `muted`) or the control.

### Menu (Mac: `KoanMenu`, the menus' `Label(…, koan:)`)

```rust
Menu::build(window, cx, |m, window, cx| {
    m.item(MenuEntry::new("play").icon(Icon::Play))
     .item(MenuEntry::new("play next").icon(Icon::PlayNext))
     .section("add to playlist")
     .submenu_icon("add to playlist", Icon::Playlist, window, cx, …)
})
```

Built from entries, separators, sections and submenus; an entry's `keys` shows the keys that do the same at the row's end, for menus drawn where there is no menu bar; the panel, rows, glyph column, check, padding and keyboard handling are the menu's. Every verb carries the glyph the Mac's menu gives it.

`control(make)` puts a control in a row of its own, in the rows' insets (a renderer's volume), and `note(text)` a sentence about the rows around it, wrapped at the Mac picker's width rather than widening the panel. `MenuSlot::trigger` opens a menu above its button (the transport's), `trigger_below` below it (a toolbar's). A menu whose content arrives after it opens (renderers answering a search) is made again in place with `refill`, reached through `MenuSlot::menu(which)`; the highlight stays on its row while the row is there. An entry that is `stays_open` leaves the menu open when chosen, for a choice among outputs or devices, where the tick and the details moving are the answer. A rich entry's second line is one line, ending in an ellipsis with the rest in its tooltip, so a long one never widens the menu. What acts on a row without choosing it is an `action`: a quiet glyph with a tooltip naming it (`lit_action` in the accent while on, as an output's EQ), or a `text_action`, a word or two ("move here"). Where a row's last action is words, a choice's tick shares that last slot with it, as what is chosen is never moved to; beside glyphs the tick keeps its own column. Ticks are in the accent, and the chosen row of a menu of choices stands on the accent at 12%, inset from the panel's edges and rounded, as the Mac's picker draws it; every row's ground is inset and rounded alike. A rich row keeps two lines' height without a second line, its title centred. A menu whose rows change while it is open keeps a `min_width`; pickers of places (outputs, devices) take `Menu::PICKER_WIDTH`, sized for a rich row with its detail and a "move here", so they hold their size. Every row keeps as many slots at its end as the most any row of the menu has, filled from the end, so the actions line up whichever rows have them.

### Controls

`ScrollArea::items(id, items).reveal(ix)` is a scrolling column whose arrow-key position is kept in view.

`Button::help` and `ToolbarButton::help` give the tooltip as the Mac's buttons do, what it does and its keys ("back (⌘[)"), in place of a glyph-only button's label.

`Button` with the spec's kinds (`Prominent`, `Standard`, `Compact`, `Bordered`, `Link`, `Text`, `Icon`, `IconOutlined`, `Toolbar`), `Toggle`, `IconToggle`, `Segmented`, `Picker`, `Slider`, `Stepper`, `TextField`, `Badge`, `Chip`, `Tooltip`, `ScrollArea` and the overlay scrollbar, and `RowList`, the virtualised list of measured rows the queue and playlists scroll in, as they are now.

A glyph-only button is made with `Button::icon_only(id, icon, help)` (or `icon_outlined`), which takes its tooltip: what it does, with its keys where it has them (koan-gui's `platform::actions::help`, "back (⌘[)"). A glyph-only button made any other way without help or a label fails a debug assertion, so none goes without saying what it does.

`Slider::glide(&Glide)`: a value that moves on its own, as a seek bar's playhead does. The owner keeps a `Glide`, sets the value at a moment and its rate (`set`) and whether it is moving (`set_moving`); on macOS the fill and the thumb are then a shader quad that reckons the value at each present, so the slider glides with no frame drawn.

`Placeholder::new(side)`: what a sleeve, photo or playlist face shows while it has none or none yet, the app icon's ensō faded back on `rule` (the Mac's `EnsoPlaceholder`), one atlas sprite per size. A record's average colour still stands in while its bitmap uploads, since that carries the record. While what it stands for is still loading (`CoverArt::Loading`, `Placeholder::loading(true)`), the same ensō draws itself: the `Spinner`, placed so its stroke lies exactly on the mark (same diameter, brush and centre) in the mark's colour, so loading that ends in no picture leaves the mark where it was. A `Cover` given an `id` fades its picture in over the spinner when it arrives after loading.

`Level::new(id, value, max)`: a whole number on a `Slider` between two glyphs (a volume's speakers by default), `numbered()` to write it after them. A drag moves only the slider and commits once, when let go or stepped by a key: for a value set over the network, where every step would be a round trip and the value reported back lags the hand.

`Spinner::new(side)`, and `.reserving(size)` to hold exactly the room of what is loading, so nothing moves when it arrives. It is the one loading mark: a pane waiting on its rows shows it rather than a word. On the shader path the ensō is drawn from a field baked once (how far along the brush and how far from it each texel is), its head and tail moving with `time`, so it turns smoothly at any size with no frame drawn.

### Animated (GPU-drawn motion)

Every aspect of the UI is smooth and moves with ease, because any animation is drawn by the GPU through shaders: at the display's refresh rate always (120 Hz on ProMotion), with next to no CPU per frame, and nothing at all while nothing moves. No animation is capped, paced or stepped by a timer.

Every animation is keyframes the GPU interpolates (#1859), as a browser's compositor does. Anything that moves (the playing bars, a seek bar's playhead, the wash's drift, a download's ring, a control's fill, the spinner) is an `AnimatedQuad`: an element drawn by a fragment program from an `Animated` handle the page writes from any thread. The handle holds sixteen plain values (colours, sizes, flags, written when they change) and eight **tracks**. A track is one segment of a moving value: from, to, when it starts, how long it takes, and its curve. The CPU writes a track when its data changes, at that data's own rate; the program evaluates it at the present's time with `koan_track(u, k, time)`, at the display's rate, with no CPU work per frame. The page and the views around it are not drawn again, and GPUI draws no frame.

```rust
static RING: ShaderProgram = ShaderProgram { name: "koan-ring", msl: r#"…"#, wgsl: r#"…"#, prepare: None };

let progress = Animated::new();                  // kept by the view, cloned to a worker
AnimatedQuad::new(&RING, &progress).size(side)   // declared in render
progress.to(0, share, Duration::from_millis(66), Ease::Linear);  // as the engine reports, ~15 Hz
```

- **`to(k, value, length, ease)`** moves track `k` from where it stands now (read on the present clock, `Animated::now()`) to `value`: a retarget mid-flight starts from what is on screen, and a spring carries its velocity on.
- **`segment(k, from, to, start, length, ease, repeat)`** writes the whole segment, for values whose start is known: an analyser frame at its own time, a playhead from its anchor, a drift that breathes (`Repeat::Alternate`) or a turn that loops (`Repeat::Loop`). **`hold(k, value)`** stands still.
- **Curves** (`Ease`): linear, ease in and out, the theme's quick-out, any CSS cubic Bézier, and a damped spring in closed form. The CPU's `Animated::at` evaluates a track exactly as the program does.
- **Rates are the engine's:** a view hands each value the engine sends to its `Animated` as the next keyframe, and the GPU interpolates. The bars' bands glide over the gap between the analyser's frames; a seek bar's playhead takes each anchor the engine reports (a seek, a pause, a skip), its downloaded part each reading of the download; the wash, the controls' fills and the spinner are written once and run by themselves. No rate is set or assumed in koan-kit or koan-gui.
- **States** arrive at once and leave over `Motion::LEAVE` (130 ms): the pointer reaching a control lights it, and its fill fades out once the pointer has gone (`transition` and `fill` in `components/mod.rs`). Every control and row follows this rule; Reduce Motion makes both instant. Text, glyphs and edges fade the same way through colour slots: a control holds one only while its leave fade runs.
- **Parking** is the kit's: a window presents while any track on screen moves and once more as the last comes to rest, then parks; a hidden or occluded one is not woken; a quad off screen is not in the scene.
- **Reduce Motion:** a `.decorative()` quad's tracks stand at their ends, a repeating one at its start. Progress is not decorative.
- **`AnimatedLayer`** moves what an element paints: its content is drawn into a texture of its own (a GPUI layer group) and composed at each present with opacity, offset and reveal tracks (`LAYER`). It is a layer only while a track moves, painted plainly once settled; shaders inside it are drawn over it at their own place. `FadeIn`, `Reveal`, `Slide` and `SplitView`'s column are built on it: a slot takes its size at once, so what is beside it moves in one step, and content sliding out takes no clicks.
- An element that lays itself out (the bars' slot, the slider's rail) calls `Animated::attach` in prepaint and `Animated::paint` in paint instead of declaring an `AnimatedQuad`.
- `SHADED` says whether the platform draws them: Metal on macOS, wgpu on Linux. Where it does not (Windows, not yet built), the component draws its fallback from `Animated::at`.

The renderer keeps the last drawn frame as layers split at the shaders, so a present composes the layers and the quads rather than drawing the scene: its cost is the quads and a few full-window copies (vendor/README.md).

### Answers (Mac: `symbolEffect(.bounce)`, the toggles' and fields' transitions)

A control answers what was done to it, by itself, so every page that uses it answers alike: `Answerer::of(id)` taken as the control draws, `start(answer, colour)` from its own handler, and `layer()` as its last child, one `ANSWER` shader quad over the control and a little past it for `Motion::SETTLE` at most, in the accent at low opacity. The clock runs only while an answer is drawn and the quad leaves the scene when it ends; under Reduce Motion, or where the renderer draws no shaders, nothing is drawn.

A change the control learns of rather than sees done (a favourite set by a shortcut or another device, music queued from a menu) answers through `follow(mark, answer, colour)` as the control draws: `mark` names the state, and a draw that finds it moved starts the answer. A control's first draw only notes the mark, so a row scrolled in shows its state still, and a change the control answered from its own handler a moment earlier is not answered twice. `announce` answers the first draw too, for something whose appearing is the news.

| Control | Answer |
|---|---|
| `Button` (glyph kinds, toolbar), `TransportControl`, `IconToggle` and `Toggle` turned on | a ring leaving the glyph or box |
| `Button` (prominent, bordered), the stepper's buttons, `CopyButton` | the ground lifting and settling |
| `Heart`, the album tile's heart included, however the favourite changed | six dots leaving a heart that fills; a ring closing in on one that empties |
| `Segmented`, a `TextField` taking the keyboard, `NavRow::accept` (a drop on a sidebar row), `NavRow::arrivals` (music sent there from a menu), `CopyText`, `NavNote::said` (a notice appearing) | a line drawn in under it |

`SplitView::shown(bool)` slides the leading column out and back, and `Slide::new(id, &state, shown, width)` does the same for a panel at a page's trailing edge (the lyrics), its owner letting the panel go once `SlideState::closed`. Both ease the width over `Motion::SETTLE` with the content laid out at full width and clipped, so nothing reflows; at once under Reduce Motion.

The storybook's motion playground plays a value on one track as a component would: what it moves (position, opacity, scale, colour), the easing (linear, in-out, quick-out, a Bézier whose handles drag on the plotted curve, a spring with its stiffness and damping), the length and the repeat, on a sample drawn at the display's rate, with a scrubber and the `Animated::segment` call to copy. It doubles as a visual test of every easing.

`PlayingBars::new(color, attach)` (Mac: `PlayingLevels`): the playing bars, `BarMetrics` in size, an animated quad drawn from the bands in a `BarLevels` that the app writes. Where the renderer draws no shaders, `attach` records each slot's `BarsPlace` and the app paints them as quads over the window's content inside `bars_overlay`. `renderer_mark(color)` takes the bars' place while the music plays on a renderer handed the file itself, where nothing passes through to measure.

`SectionHeading::new(title)` (Mac: the artist page's section titles): the heading over one of a page's sections, `body` in `strong`, its room above and below its own; `trailing` takes the section's own verbs. Every section is headed, a single one included, where the Mac's artist page heads sections only when there are several: a discography of albums alone still says what it is. A list measuring its items takes `SectionHeading::height`.

`FadeIn::new(id, element)`: what arrives after its page (a photograph, a biography) fading in over `Motion::NORMAL`, or appearing under Reduce Motion. It takes no size of its own, so the element lays out as it would unwrapped: a wrapper that fills its parent takes the whole header row from the title beside a photo.

`Reveal::new(id, size, element).arriving(true)`: room made for what arrives late, as an artist's photo does: the slot grows from nothing to `size` over `Motion::NORMAL`, moving what is beside it along, while what it holds fades in, laid out at full size and clipped rather than squeezed. Not arriving (known when the page first drew), it stands at its size. A layout change, so it runs on GPUI frames for its length; instant under Reduce Motion.

`EmptyState::new(icon, title).detail(…)` (Mac: `KoanUnavailable`), centred by the page's body.

### AvailabilityMark (Mac: `AvailabilityMark`, `TransferMeter`)

`AvailabilityMark::new(Some(Availability::…))`, `.picked(selected)`: where a track's file is (kept on the server, downloaded from it, a local file) or how its download is going (next, queued, failed, a ring filling as the bytes land), `fine`-sized and right-aligned, a word where the look hides icons. A transferring mark is handed the transfer's `Glide` (its share at the last report and its rate) and draws `ProgressRing::glide`, a shader quad reckoning the share per present, so the ring fills with no frame drawn and nothing is read while it does; where the renderer draws no shaders it draws the glide's last value each frame it shows. `Slider::reach(&Glide)` is the seek bar's counterpart: the rule dimmed past what has arrived, the arrived part drawn by the glide's own shader. Every row that says where a track is draws this one mark: the queue, the library's track rows, playlists.

### SelectionBar (Mac: iOS's `SelectionBar`, the Mac's menu on a multi-selection)

`SelectionBar::new("3 picked").verb(id, text, run).element(trigger).on_done(run)`: along the foot of a page while picking, the count leading and the verbs trailing as `Text` buttons, then done. One bar for every page that picks, so its verbs read the same everywhere.

### Banner (Mac: `ServerBanner`)

`Banner::new(icon, message, tone).action(label, run)`: a line across the top of the page column for something true of every page until it is put right, the server this device cannot use. `fine` in its tone over a rule, two lines at most, the action a compact button at the far edge.

### Record tile (Mac: `AlbumGridCell`, `AlbumCollection`'s tiles)

`TileGrid::for_width(width, scale)` lays a record grid out: `TileColumns` across, the page's margin, each row's height with its caption, and the measures a tile's owner tells its parts apart by (`caption_lines`, `TileArt::on_heart`). `TileArt::new(side, sleeve)` is the sleeve and what lies over it: the codec and the release's badge, the play mark or the starting ellipsis over a dim, the playing bars, the share on this device as a bar along the foot, the heart on its disc, and while picking the tick and the picked outline. While a sleeve is on its way the ensō draws itself (`TileSleeve::Loading`); one that arrives fades in over the record's colour, or over that ensō where no colour stood in. The tile's one interactive element and its engine wiring stay with the page.

### Lists (Mac: `KoanTable`, `NSCollectionView`)

`uniform_list` and `list` with their handles, `list_scrollbar` over either, `offset_down` for a handle's offset, `OVERDRAW` for a `list`'s layout past the screen, and `width_probe`, which keeps a page's width and redraws it when it changes. A page of `TableRow`s takes `table_list`; one laying out tiles a row at a time takes these.

### Names, tabs and status (Mac: `LinkText`, Settings' tabs, its status line)

`link_text(id, text, ink, link_action(...))` is a name that leads somewhere, underlined under the pointer; `name_line` sets a line of them on one baseline, each ending in an ellipsis rather than cut mid-glyph. `TabStrip` is a window's tabs, glyph over label, the chosen one in the accent and underlined. `StatusLine` is the one line at a window's foot that says what is under way or how it went. `Sparkline` is a small line through a series of values, an EQ curve's thumbnail. `FormRow::compact()` takes an 80-point label column for a popover's rows. `SecondaryWindow::transparent_titlebar()` runs the content under the title bar, the window drawing its top `strip_height()` itself.

### Sheet (Mac: `koanSheet`, `KoanSheetAction`)

`sheet::{panel, wide_panel, list_panel, head, head_data, search_head, band, body, foot, note, scrim}`: the square panel (`wide_panel` for a body ending in a table of paths, `list_panel` for a list to pick from), its head with the title and trailing actions, its scrolling body. `sheet::present(view, window)` shows a sheet that is a view of the app's own, one that lives while it shows (organise, previewing as it is typed), over the shell and under any question it asks, holding the keyboard while it does; `sheet::close` takes it away. `sheet::layered(Layer, element)` draws an overlay after the window's content, so a panel, a sheet or a question covers the playing bars the root paints over the page; `Layer` orders them (panel, sheet, question). `sheet::Asking` is a window's one host for the question a write asks first (`confirm` with the lines that will change, `name`), drawn over the shell. Every sheet in koan-gui is built from these.

The question host gives the keyboard back to whatever had it when it asked, once the question is answered or put away.

### DenseTable (Mac: the EQ's band table)

`table_list(id, count, rows)`: a virtualised list of `TableRow`s, made a screenful at a time, for a table too long to build whole (a library's organise plan).

`DenseTable::new(columns).header(…).rows(…)` with `Column::fixed(width)` or `Column::flex()` and an `Align` each: the compact table an editor shows, where every cell may be a field. A `DenseRow` can `span` cells (a graphic EQ's curve across four columns) and carries a `trailing` menu; `trailing_width` keeps that room on every row, the header's included, so the columns line up whichever rows have one. Columns, gutters and row height are the table's; rows declare cells.

### ProgressRow and StatusBlock (Mac: `ActivityIndicator`, `SidebarView`'s foot)

`ProgressRow::new(title).counts(…).detail(…).fraction(Some(f))` with an optional `on_stop`: one running task, its bar determinate or not, padded so the bar never meets the next row. `StatusBlock` is a sidebar's foot under a row rule: `line(icon, text, tone)` for what the device is connected to, `figure(text)` for each count beneath it. A `link` line ("connected to …") opens what its tooltip names when clicked.

### Choice menus (Mac: the EQ's pop-up menus)

`choice::menu(id, shown, Trigger::Field | Trigger::Word, items, window, cx)`: a control showing what is chosen (a field with a chevron, or a quiet word) that opens a menu panel under it, of `Item::choice` rows ticked where in force, `Item::action`, `Item::destructive`, `Item::Heading` and `Item::Rule`. The window keeps its open state under the id; escape, a press outside or a choice closes it. `Picker` stays the form's control for picking one of a list.

### QrCode (Mac: the invite and pairing codes)

`QrCode::new(dark, side)` from the modules row by row, and `element(side, margin)`: dark runs as quads on a light quiet zone in either appearance, since phones read that best.

### SecondaryWindow (Mac: Settings, the EQ window, About)

`SecondaryWindow::new(title, app_id).size(w, h).min(w, h)` or `.fixed()`, then `open(cx, build)` (or `options(cx)`): a window beside the main one, centred at its size in points, its content under the root the kit's sheets, menus and fields draw in. `MainWindow::new(title, app_id).place(saved).min(size).buttons_at(origin).open(cx, build)` is the app's own window: put back where it was left, its content under the title bar on macOS with the window's buttons on the toolbar's ground. `moves_window(area)` makes bare space move the window and zoom it on a double click, as a title bar does; where the compositor leaves the decorations to the app (`draws_decorations`), `window_buttons(on_close)` and `close_button(on_close)` are the buttons a toolbar or a top strip carries.

`opens_panel(id, trigger, content)` is a control that opens a panel under it, made each time it opens, with a `Dismiss` for its rows; `ToolbarMenu` is one with a toolbar button, `TextTrigger` the word that opens one in a form row or the selection bar. `anchored_panel` stands a panel by the control that opened it and keeps it in the window. `title_text` is a title outside a page's header (About, a confirmation). `TextArea` (with `AreaState`) is several lines of text on a field's ground. `sheet::trap_focus` keeps Tab inside a sheet its owner draws. `RowMetrics::lead_width(count)` is a number column measured in the face its numbers are drawn in.

`koan_kit::app::platform` holds what the app asks of the platform beyond its windows' content (start-up, the menu bar and the Dock's menu, the system, the keymap, a scroll's phase), `app::image` the decoded sleeves the components draw, and `app::test` the test harness. koan-gui reaches `raw` only where it draws by hand: the EQ graph and the profiler's measurements.

## Accessibility

Every koan-kit component reports itself to the accessibility tree GPUI builds over AccessKit (VoiceOver, Orca, UI Automation), so a page made from the kit is read without code of its own. The tree is built only while assistive technology is attached, so this costs nothing otherwise. Each sets its role and name beside its id (`accessible` in `components/mod.rs`) and answers a reader's click with the handler a pointer would call (`answers_click`):

| Component | Role, name and state |
|---|---|
| `Button`, `ToolbarButton`, `PlayButton`, `ShuffleButton` | Button, named by the label or a glyph's tooltip ("shuffle Hounds of Love") |
| `Toggle`, `IconToggle` | Switch, toggled |
| `Slider` (`named`), `Level` | Slider: the value, its range and step, and the low end's text as the value said ("1:00"); Increment and Decrement step it as the arrow keys do |
| `Stepper` | SpinButton, with value, range and step; Increment and Decrement |
| `Segmented` | RadioGroup of RadioButtons, the chosen one toggled, with their place in the set |
| `Picker` (`named`) | ComboBox, named by its form row's title, the choice as its value, expanded while open |
| `MenuPanel`, `MenuRow` | Menu of MenuItems, a checkbox item where it ticks, its second line as the description |
| `NavRow`, `NavHeading` | Link, selected where it is; Heading |
| `PageHeader`'s title, `SectionHeading` | Heading, levels 1 and 2 |
| `Badge` | Label |
| `ProgressRow` | ProgressIndicator, with its share and its counts |
| `EmptyState` | Group, named by what it says |
| sheets' `panel`, `Viewer` | Dialog |
| `Toolbar` | Toolbar |
| `TableRow` (`spoken`, `place`, `on_reveal`) | ListItem, or Heading for a record's heading, read as one line ("Cloudbusting, Kate Bush, Hounds of Love, 5:10, favourite"); its place in the list; selected. The links and the heart inside stay reachable as its children |
| `RowList` (`named`), `spoken_list` around a `uniform_list` | List, named for what it holds |
| `Banner`'s message, `NavNote` | Status, announced politely as it appears; Alert, announced at once, where something went wrong |

A disabled control is reported as disabled (`aria_disabled`, a `KOAN:` addition to the vendored GPUI) and drops its actions. Live regions are `aria_live`, another. An announcement carries its words as both name and value, since VoiceOver reads a live node's value and Orca its name, and sits on a leaf: a live region's descendants are live too, so a button or link inside it would be announced again. A progress row is not live; a reader reads its counts when it comes to it, rather than at every tick of a scan.

What only a reader reads, a row's spoken line, its place and its reveal, is worked out only while one is attached (`Window::is_a11y_active`; koan-gui's `Paint::a11y`), since a list's rows are made again at every scroll frame.

A virtual list draws only the rows on screen, and only rows drawn are in the tree. Each row says where it is (`place`: "row 412 of 5,000"), and when a reader moves to one it asks the list to bring it on screen (`ScrollIntoView`), which `on_reveal` answers by scrolling the row's neighbours into view (`RowListState::reveal_around`; `reveal_around` for a `uniform_list`, which keeps one scroll and so asks for the neighbour the reader is heading for), so the next row is drawn before the reader reaches it.

Pages say what no component says for them through `koan_kit::app::a11y`: the sidebar is navigation, the stage main, an album tile a link to its record, the transport's track line a polite live region.

`Placeholder` and the wash take no role and stay out of the tree. A node's id is its global element id, so components whose ids are made from content keep them stable. Still to come: text fields (gpui-component's `Input` draws them). `tests/a11y.rs` reads each component's node out of the tree, its state included, and sends a click, an increment and a scroll into view through `Window::dispatch_a11y_action` as a screen reader would; `Window::set_a11y_active_for_test` attaches one, since GPUI's test platform never does.

## Swift roles and their GPUI components

| Swift (`KoanTheme.swift`, `Views/`) | GPUI |
|---|---|
| `KoanType`, `.koanText(role, tone)` | `TypeRole`, `KoanStyled::text_role`, `Tone` |
| `KoanTheme.Space`, `spaceScale`, `sizeScale` | `Space`, `Look::space`, `Scale` |
| `KoanTheme.hairline`, `KoanDivider` | `HAIRLINE`, `MenuSeparator`; a `Divider` component (gap) |
| `Motion`, `.koanAnimation` | `Motion`, the components' `fade` |
| `KoanIcon`, `Icons.swift`, `LayerSymbol` | `KoanIcon`, `Icon`, the glyph atlas |
| `KoanLabel` | `Button`/`NavRow` with an icon; a `Label` component (gap) |
| `koanButton(kind)`, `KoanButtonStyle` | `Button`, `ButtonKind` |
| `KoanToggleStyle`, `KoanIconToggle` | `Toggle`, `IconToggle` |
| `KoanSegmentedPicker`, `KoanPicker`, `KoanListPicker` | `Segmented`, `Picker` |
| `KoanSlider`, `KoanStepper` | `Slider`, `Stepper` |
| `koanField`, `KoanSearchField` | `TextField`, `FilterField` |
| `koanBadge`, `koanChip` | `Badge`, `Chip` |
| `KoanForm`, `KoanSectionHeader`, `KoanLabeledContentStyle` | `Form`, `FormSection`, `SectionHeader`, `FormRow` |
| `KoanMenu`, `koanMenuButton` | `Menu`, `MenuEntry`, `KoanContextMenu`, `MenuPanel` |
| `koanRow`, `koanNavRow`, `koanSidebar` | `ListRow`, `NavRow` |
| `KoanTable`, `TableRow`, `TrackTableRow`, `RowMetrics` | `TableRow`, `Column`, `RowMetrics` |
| `PageHeader`, the stage's edges | `Page`, `PageHeader` |
| `.toolbar`, `ToolbarItem`, the toolbar kind | `Toolbar`, `ToolbarButton` |
| `KoanUnavailable` | `EmptyState` |
| `EnsoPlaceholder`, `AlbumArtwork`'s ground | `Placeholder` |
| `ProgressView` in the theme | `Spinner` |
| `PlayingLevels`, the playing bars | `PlayingBars`, `BarLevels` |
| `koanSheet`, `KoanSheetAction` | `sheet` parts, `Asking` |
| `ActivityIndicator` | `ProgressRow`, `ProgressBar` |
| `SidebarView`'s status foot | `StatusBlock` |
| `koanPopover` | `MenuPanel` as a popover (gap) |
| `KoanTabItem` (Settings tabs) | `settings/sheet.rs`'s tabs; a `Tabs` component (gap) |

## Where the spec and the Swift app disagree

The spec wins, except for these, each a deliberate choice for the desktop.

- **Page margins.** The spec gives 32 on the Mac; the Swift pages use 16 for lists and 20 for grids (`RowMetrics.pageEdge`, `collectionMargin`). `Page` takes one margin on every side, above the header included (`RowMetrics::page_margin`): 16, as the Swift lists use, and the tile grids take the same (`RowMetrics::page_margin_on`).
- **Toggle and form rows: 24 points, a deliberate deviation for the desktop.** The spec gives a 44-point hit area; the Mac draws its toggles 24 tall (`KoanToggleStyle.hit`), and 44-point rows made Settings looser than the Mac. Rows are 24 at desktop density, the whole row's width taking the click. 44 stays the rule for touch platforms, should koan-kit ever reach one.
- **Form label column.** Swift's is 200; koan-gui had 210. 200.

## Enforcement

Two parts, the compiler first.

**The crate boundary.** Pages are written in `koan_kit::app`, which leaves out `px`, `rgb`, `hsla`, fonts and gpui-component's widgets. koan-gui still depends on gpui-kit itself, since GPUI's derive macros name `gpui::` paths, so the boundary is a convention the check below holds rather than a compile error. Sizes reach pages as values the system computed (`RowMetrics`, `Look::space`); where a component takes a figure in points (`TableRow::figure`, `Look::spaced`), it is a column's or a gap's width the component scales, and the check does not count it.

**`just gui-theme-leaks`**, in CI's GUI job: sizes and colours written as numbers (`px(`, `rgb(`, `hsla(` and the like), methods of the `Styled` trait that fix a size (`.p_4()`, `.text_xs()`, `.w_64()`, `.line_height(`), hand-built containers in koan-gui (`TypeRole::Title` outside a `PageHeader`, `ButtonKind::Toolbar`), `gpui::` named directly, and each name taken from `koan_kit::raw`.

Pages are written in `koan_kit::app`: the app model (entities, contexts, views, events, actions, focus, globals, tasks), `div` with the traits that give it children, layout and listeners, and the platform services the app uses (the clipboard, dropped files, the file picker). Sizes, colours, fonts, images, canvases, lists, overlays and animation are not in it.

What koan-kit does not cover yet is reached through `koan_kit::raw`, by name and never by glob, each import or path under a comment saying why koan-kit cannot cover it; the check refuses a raw use without one. `crates/koan-gui/theme-leaks.toml` holds each file's count of leaks, raw names among them. The check fails when a count grows or a new file appears; an area that has moved drops its entry, and once none is left `koan_kit::raw` is deleted. The baseline only ever shrinks.
