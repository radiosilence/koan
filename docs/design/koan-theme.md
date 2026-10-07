# The kōan theme

The look koan.rocks and the web UI have, set down so a native app can be drawn in it: the tokens, and the few components every screen is made of. The Mac, iPhone and Apple TV apps are drawn in it by default (`appearance.theme = "koan"`; Settings → Appearance → Theme offers the platform's look instead); it is also the reference for a future Android app. The tokens come from `site/src/theme.css`, except where noted: `muted` is lighter here, to meet AA on `surface`, and the site should follow.

Two variants differ only in icons. **Plain** has labels alone. **With icons**, the default, has the app's icons beside them, drawn as described under [Icons](#icons). "Show icons" in Settings → Appearance (`appearance.theme_icons`) chooses between them.

## Rules

New screens follow these by default; a screen that breaks one says why in its PR.

- **One prominent action per screen.** Controls carry one of three weights: *prominent* (the accent outline, `body` type), *standard* (the label in `ink`, no outline, `control` type) and *compact* (`meta` type, tight padding). The prominent one is what the screen is for: Play on a record, play/pause in Now Playing, Sign In, the confirm on a sheet that makes or changes something. Everything else is standard, and repeated or secondary controls (favourite, ⋯, share, cancel, every control in a table) are compact. Destructive actions are compact and in `bad`. A browsing screen (search, the library root, the queue, the device tray) has no prominent action. *Why:* when every button shouts, none of them tells the eye where to go, and a row of outlined buttons reads as a form to fill in rather than a choice to make.
- **Shared edges.** A header's text starts on the same vertical edge as the content below it: the cover is the width of the album grid's first column, and the text column starts on its second. Headers are top-aligned, so the cover's top meets the title's first line, and the primary action aligns to that line too. Album, artist and playlist pages share one header layout. *Why:* aligned edges let the eye run down the page without re-finding where things begin; a centred cover beside a top-aligned title leaves a ragged gap that reads as a mistake.
- **One spacing scale.** Every gap comes from `KoanTheme.Space` (4, 8, 12, 16, 22, 32), and every rule is `KoanTheme.hairline`. Lists have no rules between rows; regions are separated by space, with at most a faint hairline. *Why:* arbitrary values drift screen by screen until nothing lines up, and a reviewer cannot tell a deliberate gap from an accident.
- **No system chrome in the kōan look.** No glass, materials, rounded inset cards or system fonts. Lists and forms sit on the theme's ground with their rows' own backgrounds given up (`washedRow()` on the list's content, which the shared containers apply). Sheets and trays take `.koanSheet()`: the ground, one header style, rows on `RowMetrics`. Navigation titles and subtitles are in Geist Mono, through the bar's appearance. Section headers are lowercase. The platform's look keeps all of the system's chrome. *Why:* one stray card, glass circle or proportional label is enough to make the theme look applied rather than designed, and every one found so far came from a platform default nobody asked for.
- **A tab's own page has no title on iOS.** The queue, library, search and settings roots leave their name to the tab bar, or the iPad's sidebar, where it is already lit; the theme's back chevron names nothing, so nothing pushed above them needs it either. A title that says more than the tab stays: a search's query, the playlist or record the queue follows. *Why:* the same word twice, a centimetre apart, is the screen telling you something it has already told you.
- **No gradient fades or scrims; the wash is the only gradient.** Bars, toolbars, panels and lists are flat. Where a bar sits on the wash (the Mac's toolbar and transport), the page stops at its edge instead of passing under it behind a fade, and legibility over the wash comes from its luminance clamp and the text tokens. A flat dim over artwork is not a fade and is allowed. `just theme-leaks` flags any gradient or mask outside the wash's own files. *Why:* a fade reads as a second, weaker wash competing with the real one, and it hides the edge it was meant to soften.
- **Dense views for technical data.** Tables of numbers (EQ filters, formats, transfer stats) use `meta` and `fine` type, compact rows, short labels with the full name in the menu or on hover, and right-aligned figures. *Why:* the people who open those views are reading values across rows, and the generous row height that suits a track list spreads a ten-band EQ over three screens.

## Tokens

### Colour

| Token | Dark | Light | Use |
|-------|------|-------|-----|
| `bg` | `#1e1e1e` | `#ffffff` | Window and page background |
| `surface` | `#2a2a2a` | `#f2f2f2` | A raised field: the search field, a sheet. Used sparingly; most separation is by rule |
| `rule` | `#383838` | `#e0e0e0` | Hairlines between rows and around regions |
| `hover` | `#4d4d4d` | `#c4c4c4` | Pointer hover and pressed fill |
| `ink` | `#cccccc` | `#333333` | Body text and icons |
| `strong` | `#ffffff` | `#111111` | Titles and the playing track's name |
| `muted` | `#919191` | `#666666` | Secondary text, unselected navigation, control outlines. The site's `#8c8c8c` is 4.3:1 on `surface` |
| `accent` | `#7dd3a7` | `#1f7a50` | Selection, the primary action, progress, the playing row, toggles on, focus. Mint is its value with nothing playing; see [Accent](#accent) |
| `bad` | `#ef6b73` | `#c43f3f` | Errors and destructive actions |

`accent` is `--color-brand` in `theme.css`. `strong` is not in `theme.css`, which has no separate title colour; the apps need one for titles over long lists.

Contrast, against `bg` unless stated (WCAG 2.2):

| Pair | Dark | Light | Rule that follows |
|------|------|-------|-------------------|
| `ink` | 10.4 | 12.6 | Body text at any size |
| `muted` | 5.3 | 5.7 | Secondary text at any size, on `bg` or `surface` (4.6 dark, 5.1 light) |
| `accent` (mint) | 9.3 | 5.3 | Text at any size. A record's accent is held to 4.5:1 or kept off text |
| `bad` | 5.6 | 5.1 | Text at any size |
| `rule` | 1.4 | 1.3 | Decoration only. Under the 3:1 a control's boundary needs, so a rule never alone marks where a control is: outlines of controls use `muted` |

### Type

One face, Geist Mono (variable, weights 100–900), bundled with the app. Sizes are points on Apple platforms and sp on Android, and each scales with the platform's text size setting from the role named.

| Role | Size | Weight | Line height | Scales with |
|------|------|--------|-------------|-------------|
| `display` | 34 | 200 | 1.1 | Large Title |
| `title` | 26 | 300 | 1.2 | Title 1 |
| `title-sm` | 22 | 300 | 1.2 | Title 2 |
| `body` | 15 | 400 | 1.45 | Body |
| `control` | 14 | 400 | 1.3 | Callout |
| `meta` | 13 | 400 | 1.35 | Subheadline |
| `fine` | 12 | 400 | 1.35 | Footnote |

`body` through `title` are the server UI's `--text-*` scale; `display` is the page title, as large as the site's but lighter.

- **Case.** Navigation, headings, buttons and labels the app writes are lowercase, as on the site. Text from the library (titles, artists, albums, genres) and from people (playlist names) keeps its own case. Accessibility labels keep proper case.
- **Numbers.** Geist Mono's figures are tabular, so durations and counts align without a separate style.
- **Truncation.** Monospace runs about 15–20 % wider than the system face at the same size: one line, truncated at the tail, for titles in rows and tiles, and two lines for titles on their own page.

### Spacing and shape

- **Spacing** in steps of 4: 4, 8, 12, 16, 22, 32. Page margins are 32 on the Mac and 22 on a phone. Rows have 11–14 vertical padding.
- **Rules** are 1 px (one device pixel on a 2× display is too faint at `rule`'s contrast; use 1 point). A selected navigation row is marked by a 2-point `accent` rule on its leading edge.
- **Corners** are square: controls, covers, sheets and the transport. The window's own corners are the system's.
- **No materials.** No blur, glass, vibrancy or shadows. A region is told apart by a rule, or rarely by `surface`. The [wash](#wash) is the one thing under the ground that is not flat.
- **Motion.** Fast and direct: a quick-out curve, a snappy start and a decisive stop, with no tail, delay, stagger or overshoot, and no springs, scale pops or glass morphs. States (pressed, hover, selection, toggles) take 80 ms (`Motion.fast`); a marker moving between places, a tab's underline, goes straight to its target in 120 ms (`Motion.normal`); the accent arriving with a record eases over 250 ms (`Motion.settle`) and never draws the eye. A tab switch swaps the page at once. With Reduce Motion all of it is instant. Components take these tokens through `.koanAnimation`, never their own `.animation`. System transitions (navigation pushes, sheets) stay as the platform draws them.

### Accent

The accent follows the record playing, and is tone-mapped the same way in both themes: the platform's look tints its controls with the same colour. Its hue is the sleeve's, from the analysis the wash already runs (`Color.dominant`: a mean of hue weighted by how colourful each sample is). Its lightness and chroma are moved into a band per appearance, in OKLCH, so a dark or muddy sleeve gives a clean, bright version of its hue and light mode never goes pastel:

| Token | Dark | Light |
|-------|------|-------|
| `accent-lightness` | 0.70–0.85 | 0.45–0.60 |
| `accent-chroma` | 0.10–0.19 | 0.10–0.19 |
| `accent-no-hue` | 0.04 | 0.04 |

- Within the band, the accent takes the most vivid lightness that reaches 4.5:1 on `bg` and `surface`: the darkest of the band in dark mode, the lightest in light.
- Where no lightness in the band does, the accent is used for fills, indicators and rings only, at 3:1, and text that would have been the accent is `ink`.
- A sleeve whose chroma is under `accent-no-hue`, and no record at all, give mint.
- The hue is never moved. A red record gives a red accent, even one close to `bad`; errors are told apart by where they appear and what they say, not by hue alone.
- A change of record eases to the new accent over 0.35 s, and only when the colour was not already known; one in hand lands with the record.
- "Colours from the record" (`appearance.record_colours`) off pins the accent to mint and removes the wash, in both themes.

### Wash

The playing record's sleeve, blurred to colour fields and drifting, behind the ground. It is the one element that is not flat, and it carries data. Surfaces stay flat tokens drawn over it; it shows where the design leaves the ground bare: the Mac's content column, and Now Playing and page backgrounds on iOS and tvOS. Nothing glass sits on it.

On the Mac, "wash the whole window" (`appearance.wash_window`) runs it under the toolbar and the sidebar as well, edge to edge from the toolbar down to the transport. Those have no ground and no material then: the sidebar's system glass is set clear, and each region meets the page at a hairline, where the page stops rather than passing beneath. The transport keeps its `bg` and spans the whole window beneath every column, as the phone's mini player does, and the sidebar, page and lyrics all stop at its rule. Off, toolbar and sidebar keep `bg` and the transport spans the page only.

Each sample of the baked sleeve is held to a luminance limit before it is drawn: in dark mode no brighter than the level at which `muted` text keeps 4.6:1, and in light mode no darker than keeps the wash, drawn over `bg`, at least as light as `surface` — so whatever reads on `surface`, the accent and `bad` included, reads over the wash. The wash so carries the record's hue and never its brightness, and every text token passes over any sleeve. `wash` is its strength, the share of the toned sleeve over `bg`: 0.6 on the Mac and iPhone, 0.5 on a television. The graphics level in Settings → Appearance governs it as in the platform's look: lower levels stop the drift, then remove the wash.

## Components

Each is described by its parts and states, so it can be built on any platform.

### Button

Three weights, assigned by the [rules](#rules); labels are lowercase and stay on one line.

- **Prominent:** the label in `accent`, a 1-point `accent` outline, `body` type, padding 12 × 20. Pressed: `hover` fill. At most one per screen.
- **Standard:** the label (and glyph) in `ink`, no outline, `control` type, padding 8 × 4.
- **Compact:** as standard in `meta` type, padding 4 × 2.
- **Bordered:** compact, in a 1-point `muted` outline, padding 6 × 12. A secondary action standing in a form row, such as copy, scan or sign out, where bare text would not read as something to press.
- **Link:** the label in `muted`, underlined, no outline; `ink` when pressed. Anything that goes somewhere (a web page, another page) and a lesser action inline with text.
- **Destructive:** compact or bordered, in `bad`.
- **Text button:** the label in `muted`, no outline, used in bars ("clear", "sleep"). Hover: `ink`.
- **Icon button** (transport): the glyph in `ink`, at least 44 × 44 pt to hit, with no outline, except play/pause, which has a square 1-point `ink` outline.
- **Disabled:** label and outline at 40 % opacity.
- **Focus** (keyboard, and tvOS): a 2-point `accent` ring outside the control. On a television, no lift, shadow or glass.

A text action that is not standard or prominent and not in a bar is bordered or underlined: bordered is a button, underlined is a link. *Why:* bare text in a form row reads as a value rather than something to press, and an underline is the long-standing sign that text goes somewhere.

### Toggle

A square box, 14 × 14 within a 44-point hit area. Off: a 1-point `muted` outline. On: filled `accent` with a `bg`-coloured check. The label leads, in `body`, and the box sits at the row's trailing edge, where a switch would.

### Segmented control

A row of text options, `control` type, 18 apart. Unselected: `muted`. Selected: `ink`, underlined with a 1-point `accent` line 5 points below the baseline. No track and no background.

### Form row

The label leads in `body`, `ink`, lowercase, whether the row is a toggle, a picker, a stepper or a field; a value it shows trails in `control`, `muted`. A label that is data rather than the app's words (an AutoEQ maker, a server extension) keeps its case. A pop-up picker's value is a menu: the chosen option in `control`, `ink`, with a chevron, and no bezel.

### Slider

A 1-point `rule` track with a 3-point `accent` fill up to the value, and a square 8 × 8 `ink` thumb shown only on hover, focus or drag. The hit area is 44 points tall. Times or values sit at either end in `fine`, `muted`.

### List row

The title in `body`, `ink`; secondary text in `meta`, `muted`; numbers right-aligned in `meta`, `muted`. No rule between rows in the native apps: rows are told apart by their rhythm and alignment, which a grey grid over the wash only obscures. Where a region would otherwise run into the next, a faint hairline of `ink` at 12 % opacity (`koanRowRule`) may mark it. Outlines stay on prominent buttons and in the web UI. Hover: `hover` at 30 % behind the row. The playing row's title and number are `accent`. A selected row has a `surface` fill.

### Navigation row (sidebar)

The label in `body`, `muted`, lowercase; with icons, the glyph before it in the same colour. Selected: `accent`, with the 2-point leading rule. On the Mac the list keeps AppKit's own selection beneath it, which is what VoiceOver announces and the arrow keys move. Section headings in `fine`, `ink`, with 16 points above.

### Tab bar (phone)

Flat and full-width, with a 1-point `rule` along its top and `bg` beneath, 64 points tall. Labels in `fine`, lowercase; with icons, a glyph above each. Unselected: `muted`. Selected: `accent`, the label underlined. The mini player sits directly above it as a row with its own top rule, whose first part is the playhead in a 2-point `accent` line.

### Select bar (phone)

Select mode begins from a "select" text button in a page's bar (in the queue's header, which has no bar). Rows take the List's ticks and tiles a ring; a bar along the foot of the page, over the mini player, says how many are picked with "done" beside it, and below, the verbs as the tab bar's items are drawn: play, play next, add to queue, add to playlist, favourite, and remove where the page holds the things picked (the queue, a playlist, history). Each verb ends the mode. VoiceOver reads each tick as selected or not, and each verb by its full name.

### Transport (Mac)

A full-width bar, 68 points tall, with a 1-point `rule` along its top and no shadow, material or scrim. With the wash under the whole window it runs under the sidebar and lyrics too, and every column above ends at its rule. Left: the cover, square, 40 points, then the title in `meta`, `strong` and the artist and album in `fine`, `muted`. Centre: shuffle, previous, play/pause, next and repeat as icon buttons, over the slider with times at either end. Right: the format badge (`fine`, a 1-point `rule` outline, `ink`), then sleep, output and lyrics, as text buttons in the plain variant or icon buttons with icons.

## Icons

With icons (`appearance.theme_icons = true`), each navigation row, tab, transport control and action has its glyph. Every glyph is drawn the same way: a single line, `ultraLight` to `light` weight (1.2–1.4 points at 15–20 points), monochrome, in the colour of the text beside it. No fills, no hierarchical or palette colour, no badges. Glyphs never replace a label that the plain variant shows.

The Apple apps keep the SF Symbols they name today (`Icon.*`). On Android, Material Symbols Outlined at weight 200, or Lucide, with one glyph per role mapped from the same names.

## On the platforms

- **macOS, iOS and tvOS:** `Support/KoanTheme.swift`. Views name roles and never a colour, font, corner or material:
  - text: `.koanText(role, tone)`, or `Font.role(_:system:)` and `KoanTheme.style(_:system:)` where a view takes a font or style; `.koanCase()` lowercases a title on screen, and `KoanTheme.label(_:)` the bare strings (navigation titles, AppKit labels) that cannot be;
  - motion: `KoanTheme.Motion` through `.koanAnimation(_:value:)`;
  - spacing: `KoanTheme.Space` and `KoanTheme.hairline`;
  - ground: `.koanSurface()`, `.koanRule()`, `.koanBar(radius:inset:)` for the transport, `.koanToolbar(glass:)`, `.koanSidebar()`, `.koanSheet()`;
  - controls: `.koanButton(kind)`, or `.koanButton(kind, system:)` where the platform's look had a style of its own, and `.koanButtons(kind)` for a group, in the theme only; `.koanToggle()`, `KoanSegmentedPicker`, `KoanPicker` for pop-up pickers (a menu in the theme, since AppKit's pop-up button and UIKit's picker ignore the theme's type), `KoanSlider`, `KoanStepper`, `.koanControl()` for other menus, `.koanField()`, `.koanChip()`, `.koanBadge()`; toolbar items leave their glass panes through `KoanTheme.pane(_:)`;
  - lists and forms: `.koanList()` (every list in the wash takes it through `washedGround()`) and `washedRow()` on a list's content, since a row's background is set per row and a list does not pass one down, `.koanNavRow(selected:)`, `KoanForm` (a form; in the theme, square sections without cards: stacked on the Mac and tvOS, a grouped list on iOS; on tvOS each row is one of the theme's controls), `KoanSectionHeader`, `KoanDivider`; `.koanRow(selected:)` for a row drawn in SwiftUI, which the Mac's AppKit tables are not;
  - pieces: `KoanLabel(title, icon:)` for every label with an icon, `KoanTabItem` for the phone's tab bar, `KoanUnavailable` for an empty page;
  - shape: `KoanTheme.radius(_:)` and `KoanTheme.shadow(_:)`, which give square corners and no shadow in the theme;
  - focus on tvOS: `.koanFocus()`, which the television's own button and row styles (`TelevisionButton`, `TelevisionRow`) draw in the theme in place of the system's platter.

  Each draws the platform's look exactly as before when the theme is off, so the "system" theme is the same roles with different answers. The accent is the environment's tint (`roomTint` for layer-drawn views), with `koanAccent` beside it saying whether it reads as text; whether icons are drawn is `koanIcons`, set at each scene's root by `.koanTheme(_:)`. AppKit-drawn views read `NSColor.koan*` (`koanBad`, `koanSeparator` and `koanSelection` among them) and `NSFont.role`. The platform's glass becomes `surface` in the theme through `.glass(_:fallback:in:)`, a material through `.koanMaterial(_:in:)`, and a popover's ground through `.koanPopover()`; the album grid's AppKit heart draws its own flat ground. The Mac's tables keep AppKit's selection fill. The setting is `appearance.theme`, `"koan"` (the default) or `"system"`, read at launch; the Theme picker in Settings → Appearance writes it, and "Show icons" appears there only with the kōan theme. `just theme-leaks` finds styling that bypasses the roles.
- **Android:** the colour tokens map to a Material 3 `ColorScheme` (`bg` → `background`, `surface` → `surface`, `accent` → `primary`, `ink` → `onBackground`, `muted` → `onSurfaceVariant`, `rule` → `outlineVariant`, `bad` → `error`), the type roles to `Typography`, and `Shapes` are all zero-radius. The components above replace Material's own where they differ: outlined rather than filled buttons, the underlined segmented control, and the flat tab bar.
