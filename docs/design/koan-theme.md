# The kōan theme

The look koan.rocks and the web UI have, set down so a native app can be drawn in it: the tokens, and the few components every screen is made of. The Mac, iPhone and Apple TV apps are drawn in it by default (`appearance.theme = "koan"`; Settings → Appearance → Theme offers the platform's look instead); it is also the reference for a future Android app. The tokens come from `site/src/theme.css`, except where noted: `muted` is lighter here, to meet AA on `surface`, and the site should follow.

Two variants differ only in icons. **Plain** has labels alone. **With icons**, the default, has the app's icons beside them, drawn as described under [Icons](#icons). "Show icons" in Settings → Appearance (`appearance.theme_icons`) chooses between them.

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
| `accent-bad-gap` | 25° | 25° |

- Within the band, the accent takes the most vivid lightness that reaches 4.5:1 on `bg` and `surface`: the darkest of the band in dark mode, the lightest in light.
- Where no lightness in the band does, the accent is used for fills, indicators and rings only, at 3:1, and text that would have been the accent is `ink`.
- A sleeve whose chroma is under `accent-no-hue`, and no record at all, give mint.
- A hue within `accent-bad-gap` of `bad`'s moves to the edge of that gap, so a red record never reads as an error.
- A change of record eases to the new accent over 0.35 s, and only when the colour was not already known; one in hand lands with the record.
- "Colours from the record" (`appearance.record_colours`) off pins the accent to mint and removes the wash, in both themes.

### Wash

The playing record's sleeve, blurred to colour fields and drifting, behind the ground. It is the one element that is not flat, and it carries data. Surfaces stay flat tokens drawn over it; it shows where the design leaves the ground bare: the Mac's content column, and Now Playing and page backgrounds on iOS and tvOS. Nothing glass sits on it.

Each sample of the baked sleeve is held to a luminance limit before it is drawn: in dark mode no brighter than the level at which `muted` text keeps 4.6:1, and in light mode no darker than keeps the wash, drawn over `bg`, at least as light as `surface` — so whatever reads on `surface`, the accent and `bad` included, reads over the wash. The wash so carries the record's hue and never its brightness, and every text token passes over any sleeve. `wash` is its strength, the share of the toned sleeve over `bg`: 0.6 on the Mac and iPhone, 0.5 on a television. The graphics level in Settings → Appearance governs it as in the platform's look: lower levels stop the drift, then remove the wash.

## Components

Each is described by its parts and states, so it can be built on any platform.

### Button

- **Primary:** the label in `accent`, a 1-point `accent` outline, `control` type, padding 10 × 16. Pressed: `hover` fill.
- **Secondary:** the label in `ink`, a 1-point `muted` outline. Never a `rule` outline, which is under 3:1.
- **Text button:** the label in `muted`, no outline, used in bars ("clear", "sleep"). Hover: `ink`.
- **Icon button** (transport): the glyph in `ink`, at least 44 × 44 pt to hit, with no outline, except play/pause, which has a square 1-point `ink` outline.
- **Disabled:** label and outline at 40 % opacity.
- **Focus** (keyboard, and tvOS): a 2-point `accent` ring outside the control. On a television, no lift, shadow or glass.

### Toggle

A square box, 14 × 14 within a 44-point hit area. Off: a 1-point `muted` outline. On: filled `accent` with a `bg`-coloured check. The label sits to its trailing side in `body`.

### Segmented control

A row of text options, `control` type, 18 apart. Unselected: `muted`. Selected: `ink`, underlined with a 1-point `accent` line 5 points below the baseline. No track and no background.

### Slider

A 1-point `rule` track with a 3-point `accent` fill up to the value, and a square 8 × 8 `ink` thumb shown only on hover, focus or drag. The hit area is 44 points tall. Times or values sit at either end in `fine`, `muted`.

### List row

The title in `body`, `ink`; secondary text in `meta`, `muted`; numbers right-aligned in `meta`, `muted`. A 1-point `rule` below each row, inset to the content's leading edge. Hover: `hover` at 30 % behind the row. The playing row's title and number are `accent`. A selected row has a `surface` fill.

### Navigation row (sidebar)

The label in `body`, `muted`, lowercase; with icons, the glyph before it in the same colour. Selected: `accent`, with the 2-point leading rule. On the Mac the list keeps AppKit's own selection beneath it, which is what VoiceOver announces and the arrow keys move. Section headings in `fine`, `ink`, with 16 points above.

### Tab bar (phone)

Flat and full-width, with a 1-point `rule` along its top and `bg` beneath, 64 points tall. Labels in `fine`, lowercase; with icons, a glyph above each. Unselected: `muted`. Selected: `accent`, the label underlined. The mini player sits directly above it as a row with its own top rule, whose first part is the playhead in a 2-point `accent` line.

### Transport (Mac)

A full-width bar, 68 points tall, with a 1-point `rule` along its top and no shadow or material. Left: the cover, square, 40 points, then the title in `meta`, `strong` and the artist and album in `fine`, `muted`. Centre: shuffle, previous, play/pause, next and repeat as icon buttons, over the slider with times at either end. Right: the format badge (`fine`, a 1-point `rule` outline, `ink`), then sleep, output and lyrics, as text buttons in the plain variant or icon buttons with icons.

## Icons

With icons (`appearance.theme_icons = true`), each navigation row, tab, transport control and action has its glyph. Every glyph is drawn the same way: a single line, `ultraLight` to `light` weight (1.2–1.4 points at 15–20 points), monochrome, in the colour of the text beside it. No fills, no hierarchical or palette colour, no badges. Glyphs never replace a label that the plain variant shows.

The Apple apps keep the SF Symbols they name today (`Icon.*`). On Android, Material Symbols Outlined at weight 200, or Lucide, with one glyph per role mapped from the same names.

## On the platforms

- **macOS, iOS and tvOS:** `Support/KoanTheme.swift`. Views name roles and never a colour, font, corner or material:
  - text: `.koanText(role, tone)`, or `Font.role(_:system:)` and `KoanTheme.style(_:system:)` where a view takes a font or style; `.koanCase()` lowercases a title on screen, and `KoanTheme.label(_:)` the bare strings (navigation titles, AppKit labels) that cannot be;
  - motion: `KoanTheme.Motion` through `.koanAnimation(_:value:)`;
  - spacing: `KoanTheme.Space` and `KoanTheme.hairline`;
  - ground: `.koanSurface()`, `.koanRule()`, `.koanBar(radius:inset:)` for the transport, `.koanToolbar(glass:)`, `.koanSidebar()`, `.koanSheet()`;
  - controls: `.koanButton(kind)`, or `.koanButton(kind, system:)` where the platform's look had a style of its own, and `.koanButtons(kind)` for a group, in the theme only; `.koanToggle()`, `KoanSegmentedPicker`, `.koanControl()` for pop-up pickers, menus and steppers, `.koanField()`, `.koanChip()`, `.koanBadge()`; toolbar items leave their glass panes through `KoanTheme.pane(_:)`;
  - lists and forms: `.koanList()` (every list in the wash takes it through `washedGround()`), `.koanNavRow(selected:)`, `KoanForm` (a form; on the Mac in the theme, sections stacked without AppKit's cards), `KoanSectionHeader`, `KoanDivider`; `.koanRow(selected:)` for a row drawn in SwiftUI, which the Mac's AppKit tables are not;
  - pieces: `KoanLabel(title, icon:)` for every label with an icon, `KoanTabItem` for the phone's tab bar, `KoanUnavailable` for an empty page;
  - shape: `KoanTheme.radius(_:)` and `KoanTheme.shadow(_:)`, which give square corners and no shadow in the theme;
  - focus on tvOS: `.koanFocus()`.

  Each draws the platform's look exactly as before when the theme is off, so the "system" theme is the same roles with different answers. The accent is the environment's tint (`roomTint` for layer-drawn views), with `koanAccent` beside it saying whether it reads as text; whether icons are drawn is `koanIcons`, set at each scene's root by `.koanTheme(_:)`. AppKit-drawn views read `NSColor.koan*` (`koanBad`, `koanSeparator` and `koanSelection` among them) and `NSFont.role`. The platform's glass becomes `surface` in the theme through `.glass(_:fallback:in:)`, a material through `.koanMaterial(_:in:)`, and a popover's ground through `.koanPopover()`; the album grid's AppKit heart draws its own flat ground. The Mac's tables keep AppKit's selection fill. The setting is `appearance.theme`, `"koan"` (the default) or `"system"`, read at launch; the Theme picker in Settings → Appearance writes it, and "Show icons" appears there only with the kōan theme. `just theme-leaks` finds styling that bypasses the roles.
- **Android:** the colour tokens map to a Material 3 `ColorScheme` (`bg` → `background`, `surface` → `surface`, `accent` → `primary`, `ink` → `onBackground`, `muted` → `onSurfaceVariant`, `rule` → `outlineVariant`, `bad` → `error`), the type roles to `Typography`, and `Shapes` are all zero-radius. The components above replace Material's own where they differ: outlined rather than filled buttons, the underlined segmented control, and the flat tab bar.
