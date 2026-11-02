# koan-kit: the design system

How the desktop app stays consistent: one design system, the one the Mac app already has (`docs/design/koan-theme.md` and the roles in `apps/macos/Sources/Koan/Support/KoanTheme.swift`), rebuilt for GPUI role for role. Pages declare what they show; where it sits, how big it is and what colour it takes are decided below them.

## Why

The Mac app got its consistency from SwiftUI's containers. `Form`, `Section`, `List`, `.toolbar` and the environment decide spacing, row metrics and type, and `KoanTheme` restyles those containers once. A page written against them cannot drift, because the spacing is not its to choose. GPUI has no such containers: every `div()` takes any padding, size and colour, so a page built by hand chooses all three and drifts. The foundation puts the containers back.

## Crates

```
gpui-kit  ->  koan-kit  ->  koan-gui
```

**koan-kit** (`crates/koan-kit`) is the design system: the theme (tokens, type roles, Geist Mono, the icon set, the accent, the wash), every control and container below, the placeholder and spinner, and the role map in this document. It knows nothing of the engine or kōan's data (no `koan-ffi`, no `koan-core`), so it builds and is tested alone, and its example is the theme sheet (today's `examples/theme_sheet.rs`), which draws every component offscreen.

**koan-gui** builds pages from koan-kit only. It does not depend on `gpui-kit`: koan-kit re-exports what pages need to be views (entities, contexts, windows, tasks, actions, key bindings, `div`, `list`, `uniform_list`, `img`, events, drag and drop, the `Styled` and `ParentElement` traits) and leaves out what would let a page choose a look (`px`, `rems`, `rgb`, `rgba`, `hsla`, `Hsla` construction, `font`, fonts and text styles, gpui-component's widgets). Going around the system is then a compile error rather than a lint. What a type cannot stop, a method on a re-exported trait, the lint below catches.

## Beyond kōan

koan-kit is written to be used by other projects (sbr, say) and published to crates.io later, so it stays project-agnostic: no koan-gui types, none of the app's wording, routes or data. Words a component draws come from its caller; a cover is `CoverArt`, never a library record. The kōan look is its default; an app supplies its own accent (the theme's `AccentFeed` takes any colour) and, in time, its own icon set through the asset source. It has a standalone example (the theme sheet, which draws every component), its own docs, and tests of its own.

One thing does not travel: the GPUI patch that stops an idle window waking at every display refresh (#1614) is vendored in this workspace through `[patch.crates-io]`, which applies to this workspace alone. Another project using koan-kit gets GPUI as published. Upstreaming the patch is the real fix.

## Built without the Mac's glitches

The Swift app fixed its glitches one at a time: a seek bar that snapped back, menus that reopened when their button was clicked to close them, hover jank, scroll jumps, layout shifts on load, covers jumping while a window resized. Each koan-kit component is built from its Swift counterpart's history (`git log -p` on the view, and the merged PRs that fixed it), so it has those fixes by construction, with a test where one can be written, and its docs list them. Two so far:

- **Seeking.** A seek lands at once and stale playhead anchors wait until the engine reaches it, or two seconds pass (`PlayerModel.swift`'s pending seek). Shared by every app in koan-ffi's published state, and tested there.
- **Menus and popovers toggle.** A trigger whose menu is open closes it, including when the click outside that dismisses the menu and the trigger's own click arrive in one event. Part of the anchor every menu and popover trigger uses.

## Layers

| Layer | Where | Owns |
|---|---|---|
| Tokens | `koan-kit/theme/tokens.rs` | The palette, `TypeRole` (sizes, weights, line heights), `Space`, `Scale` (text size), `Motion`, `HAIRLINE`, `HIT` |
| Environment | `koan-kit/theme`: `Look::of(window, cx)` | The tokens resolved for this window: appearance, text size, icons on or off, the record's accent |
| Roles | `koan-kit/theme` | `text_role`, `Tone`, `label()` (app words lowercase), `KoanIcon` and the glyph set |
| Controls | `koan-kit/components` | One interactive thing each: buttons, toggles, fields, sliders, steppers, pickers, badges, spinner, placeholder |
| Containers | `koan-kit/components` | Layout: page, page header, toolbar, form, table and rows, menu, empty state, sheet, scroll area |
| Pages | koan-gui: `library/`, `collections/`, `queue/`, `settings/`, `dsp/`, `shell.rs`, `sidebar.rs`, `transport.rs` | Content only, composed from containers and controls, and the engine's data |

Each layer reads only the ones above it. A page never reaches into tokens for a number.

## How theme values reach components

As SwiftUI's environment reaches a view: from one place, inherited, never passed by hand. Every component calls `Look::of(window, cx)` in its `render`. The window's appearance, the text size, "show icons" and the accent all live there. A component takes semantic parameters (a `Tone`, a `TypeRole`, a `RowKind`, a `ButtonKind`) and never a colour, a size in points or a font. A cached view that draws the accent calls `theme::observe_accent` so an accent change reaches it. Nothing in a page computes a size from the scale; the containers do (`RowMetrics`, `Look::space`, `Look::sized`).

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
        .actions(verbs)                       // optional, under the detail
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

`FilterField::new(&input)`: no box, the search glyph and, 10 points after it, a lowercase prompt in `muted` that names what it narrows ("filter albums"); 167 points from the glyph to the end of the text at 100 %, as the Mac's field. The input takes the rest of the field's width: given only its content's, it is zero wide, shows no prompt and takes no click. The page owns the `InputState` and what typing does.

### Table, TableRow and RowMetrics (Mac: `KoanTable`, `TableRow`, `TrackTableRow`, `RowMetrics`)

```rust
TableRow::new(id, RowKind::Art)
    .selected(picked)
    .current(playing)
    .lead(Lead::Number(n))       // or Lead::Time, Lead::Bars
    .sleeve(sleeve)              // drawn with Placeholder while missing
    .title(title, credit_line)   // flexes and truncates
    .mark(availability)
    .heart(favourite)
    .figure(Column::Format, quality)   // fixed, right-aligned
    .figure(Column::Length, duration)
```

`RowMetrics` gives heights (line 24, text 34, art 40, times the text size, plus 4 above and below), the sleeve (32), the page margin and the gap between columns. Columns are defined once (`Column::Format`, `Column::Length`, `Column::Codec`, …), each a width and an alignment, so the same figure lines up in every list. Rows have no hover fill (the user's call); a picked row is `selection` with its text in `on_selection`, and the current row is in the accent. A table is a `uniform_list` of rows inside `Page`'s body; `RowMetrics::page_margin` is the row's inset, so content lines up under the header.

### Form, FormSection, FormRow (Mac: `KoanForm`, `KoanSectionHeader`, `KoanLabeledContentStyle`)

```rust
Form::new("playback")
    .section(FormSection::titled("output").row(FormRow::new("device").control(picker)))
    .section(FormSection::new().row(toggle).footer("Why this matters."))
```

`Form` scrolls, padded 22, sections 16 apart. `FormSection` puts its header (`fine`, `ink`, 16 above) over its rows 8 apart and its footer (`fine`, `muted`). `FormRow` is the Mac's two columns: the label in `body` in a 200-point column, then the value (`control`, `muted`) or the control.

### Menu (Mac: `KoanMenu`, the menus' `Label(…, koan:)`)

```rust
Menu::build(window, cx, |m, window, cx| {
    m.item(MenuEntry::new("play").icon(Icon::Play))
     .item(MenuEntry::new("play next").icon(Icon::PlayNext))
     .section("add to playlist")
     .submenu_icon("add to playlist", Icon::Playlist, window, cx, …)
})
```

Built from entries, separators, sections and submenus; the panel, rows, glyph column, check, padding and keyboard handling are the menu's. Every verb carries the glyph the Mac's menu gives it.

### Controls

`Button` with the spec's kinds (`Prominent`, `Standard`, `Compact`, `Bordered`, `Link`, `Text`, `Icon`, `IconOutlined`, `Toolbar`), `Toggle`, `IconToggle`, `Segmented`, `Picker`, `Slider`, `Stepper`, `TextField`, `Badge`, `Chip`, `Tooltip`, `ScrollArea` and the overlay scrollbar, as they are now.

`Slider::glide(&Glide)`: a value that moves on its own, as a seek bar's playhead does. The owner keeps a `Glide`, sets the value at a moment and its rate (`set`) and whether it is moving (`set_moving`); on macOS the fill and the thumb are then a shader quad that reckons the value at each present, so the slider glides with no frame drawn.

`Placeholder::new(side)`: what a sleeve, photo or playlist face shows while it has none or none yet, the app icon's ensō faded back on `rule` (the Mac's `EnsoPlaceholder`), one atlas sprite per size. A record's average colour still stands in while its bitmap uploads, since that carries the record.

`Spinner::new(side)`, and `.reserving(size)` to hold exactly the room of what is loading, so nothing moves when it arrives.

### Animated (GPU-drawn motion)

Anything that moves every frame (the playing bars, a seek bar's playhead, the wash's drift, a download's ring, the spectrum) is an `AnimatedQuad`: an element drawn by a fragment program from sixteen uniforms, which the page writes through an `Animated` handle from any thread. The frame is presented again with the uniforms as they are then; the page and the views around it are not drawn again, and GPUI draws no frame.

```rust
static RING: ShaderProgram = ShaderProgram { name: "koan-ring", msl: r#"…"#, wgsl: r#"…"#, prepare: None };

let progress = Animated::new();            // kept by the view, cloned to a worker
AnimatedQuad::new(&RING, &progress).size(side)   // declared in render
progress.set(0, fraction);                 // from any thread, as the engine reports
```

- **Driven by values:** `set`/`set_all` write uniforms; a change wakes the window to present at its next refresh, and nothing runs between changes.
- **Driven by the clock:** `run(true)` presents every refresh while it runs, and `time` in the program counts from `set_origin`. A seek bar sets the anchor's share, rate and instant and runs while playing; an indeterminate ring runs while shown.
- **Parking** is the kit's: a window with nothing changing or running parks; a hidden or occluded one is not woken; a quad off screen is not in the scene.
- **Reduce Motion:** `.decorative()` stops the clock while the person asks for less motion. Progress is not decorative.
- **Per-present work:** a program's `prepare` runs on the CPU once per quad at each present, for what every pixel would otherwise work out alike (the wash's pose and eased opacity).
- An element that lays itself out (the bars' slot, the slider's rail) calls `Animated::attach` in prepaint and `Animated::paint` in paint instead of declaring an `AnimatedQuad`.
- `SHADED` says whether the platform draws them (macOS now; Linux once the wgpu renderer does). Where it does not, the component draws its fallback.

The renderer keeps the last drawn frame as layers split at the shaders, so a present composes the layers and the quads rather than drawing the scene: its cost is the quads and a few full-window copies (vendor/README.md).

`PlayingBars::new(color, attach)` (Mac: `PlayingLevels`): the playing bars, `BarMetrics` in size, an animated quad drawn from the bands in a `BarLevels` that the app writes. Where the renderer draws no shaders, `attach` records each slot's `BarsPlace` and the app paints them as quads over the window's content inside `bars_overlay`.

`SectionHeading::new(title)` (Mac: the artist page's section titles): the heading over one of a page's sections, `body` in `strong`, its room above and below its own; `trailing` takes the section's own verbs. Every section is headed, a single one included. A list measuring its items takes `SectionHeading::height`.

`FadeIn::new(id, element)`: what arrives after its page (a photograph, a biography) fading in over `Motion::NORMAL`, or appearing under Reduce Motion. It takes no size of its own, so the element lays out as it would unwrapped: a wrapper that fills its parent takes the whole header row from the title beside a photo.

`EmptyState::new(icon, title).detail(…)` (Mac: `KoanUnavailable`), centred by the page's body.

### Sheet (Mac: `koanSheet`, `KoanSheetAction`)

`sheet::{panel, head, head_data, body, note}`: the square panel, its head with the title and trailing actions, its scrolling body. `sheet::Asking` is a window's one host for the question a write asks first (`confirm` with the lines that will change, `name`), drawn over the shell. Every sheet in koan-gui is built from these.

### DenseTable (Mac: the EQ's band table)

`DenseTable::new(columns).header(…).rows(…)` with `Column::fixed(width)` or `Column::flex()` and an `Align` each: the compact table an editor shows, where every cell may be a field. A `DenseRow` can `span` cells (a graphic EQ's curve across four columns) and carries a `trailing` menu. Columns, gutters and row height are the table's; rows declare cells.

### ProgressRow and StatusBlock (Mac: `ActivityIndicator`, `SidebarView`'s foot)

`ProgressRow::new(title).counts(…).detail(…).fraction(Some(f))` with an optional `on_stop`: one running task, its bar determinate or not, padded so the bar never meets the next row. `StatusBlock` is a sidebar's foot under a row rule: `line(icon, text, tone)` for what the device is connected to, `figure(text)` for each count beneath it.

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

The spec wins, except where noted for the user's call.

- **Page margins.** The spec gives 32 on the Mac; the Swift pages use 16 for lists and 20 for grids (`RowMetrics.pageEdge`, `collectionMargin`). `Page` takes one margin on every side, above the header included (`RowMetrics::page_margin`), 16 for now by the user's call.
- **Toggle and form rows: 24 points, a deliberate deviation for the desktop.** The spec gives a 44-point hit area; the Mac draws its toggles 24 tall (`KoanToggleStyle.hit`), and 44-point rows made Settings looser than the Mac. Rows are 24 at desktop density, the whole row's width taking the click. 44 stays the rule for touch platforms, should koan-kit ever reach one.
- **Form label column.** Swift's is 200; koan-gui had 210. 200.

## Enforcement

Two parts, the compiler first.

**The crate boundary.** koan-gui cannot name `px`, `rgb`, `hsla`, fonts or gpui-component's widgets, because koan-kit does not re-export them. Sizes reach pages only as values the system computed (`RowMetrics`, `Look::space`), never as numbers.

**`just gui-theme-leaks`**, in CI's GUI job, for what the compiler cannot see: methods of the re-exported `Styled` trait that fix a size or colour (`.p_4()`, `.text_xs()`, `.w_64()`, `.text_color(` with anything but a `Look` colour, `.line_height(`), and hand-built containers in koan-gui (`TypeRole::Title` outside a `PageHeader`, `ButtonKind::Toolbar`, a `uniform_list(` whose rows are not `TableRow`s, a menu panel outside `Menu`).

Pages are written in `koan_kit::app`: the app model (entities, contexts, views, events, actions, focus, globals, tasks), `div` with the traits that give it children, layout and listeners, and the platform services the app uses (the clipboard, dropped files, the file picker). Sizes, colours, fonts, images, canvases, lists, overlays and animation are not in it.

What koan-kit does not cover yet is reached through `koan_kit::raw`, by name and never by glob, each import or path under a comment saying why koan-kit cannot cover it; the check refuses a raw use without one. `crates/koan-gui/theme-leaks.toml` holds each file's count of leaks, raw names among them. The check fails when a count grows or a new file appears; an area that has moved drops its entry, and once none is left `koan_kit::raw` is deleted. The baseline only ever shrinks.

## Order of work

1. This document, reviewed.
2. The move: `src/theme` and `src/components` out of koan-gui into `crates/koan-kit`, mechanical, no behaviour change. koan-gui depends on koan-kit and, for now, on `koan_kit::raw` for everything it used from `gpui-kit` directly.
3. In koan-kit, hal: tokens and roles audited against the spec, `Form`/`FormSection`/`FormRow`, `Menu` sections and icons, `Placeholder`, `Spinner`, `EmptyState`, `Sheet`, the leak check with its baseline, the role map kept here. carol: `Page`/`PageHeader`, `Toolbar`/`ToolbarButton`, `FilterField`, `TableRow`/`Column`/`RowMetrics`, and the library rows and the queue moved onto them.
4. Everyone moves their area onto the foundation, driving their entries in the baseline and `koan_kit::raw` to nothing.
