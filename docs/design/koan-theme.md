# The kōan theme

The look koan.rocks and the web UI have, set down so a native app can be drawn in it: the tokens, and the few components every screen is made of. The Mac app follows it when `appearance.theme = "koan"`; it is also the reference for a future Android app. The tokens come from `site/src/theme.css`, and where the two disagree, that file wins and this one is wrong.

Two variants differ only in icons. **Plain** (Option B in #894) has labels alone. **With icons** (Option C) has the app's icons beside them, drawn as described under [Icons](#icons). `appearance.theme_icons` chooses between them.

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
| `muted` | `#8c8c8c` | `#666666` | Secondary text, unselected navigation, control outlines |
| `brand` | `#7dd3a7` | `#1f7a50` | Selection, the primary action, progress, the playing row |
| `bad` | `#ef6b73` | `#c43f3f` | Errors and destructive actions |

`strong` is not in `theme.css`, which has no separate title colour; the apps need one for titles over long lists.

Contrast, against `bg` unless stated (WCAG 2.2):

| Pair | Dark | Light | Rule that follows |
|------|------|-------|-------------------|
| `ink` | 10.4 | 12.6 | Body text at any size |
| `muted` | 5.0 | 5.7 | Secondary text on `bg`. On `surface` it is 4.3 dark (5.1 light), under 4.5: muted text sits on `bg`, or is 18 pt or larger |
| `brand` | 9.3 | 5.3 | Text at any size |
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
- **Rules** are 1 px (one device pixel on a 2× display is too faint at `rule`'s contrast; use 1 point). A selected navigation row is marked by a 2-point `brand` rule on its leading edge.
- **Corners** are square: controls, covers, sheets and the transport. The window's own corners are the system's.
- **No materials.** No blur, glass, vibrancy or shadows. A region is told apart by a rule, or rarely by `surface`.
- **Motion.** None of its own: selection, hover and pressed states change at once. System transitions (navigation pushes, sheets) stay as the platform draws them.

## Components

Each is described by its parts and states, so it can be built on any platform.

### Button

- **Primary:** the label in `brand`, a 1-point `brand` outline, `control` type, padding 10 × 16. Pressed: `hover` fill.
- **Secondary:** the label in `ink`, a 1-point `muted` outline. Never a `rule` outline, which is under 3:1.
- **Text button:** the label in `muted`, no outline, used in bars ("clear", "sleep"). Hover: `ink`.
- **Icon button** (transport): the glyph in `ink`, at least 44 × 44 pt to hit, with no outline, except play/pause, which has a square 1-point `ink` outline.
- **Disabled:** label and outline at 40 % opacity.
- **Focus** (keyboard): a 2-point `brand` outline outside the control.

### Toggle

A square box, 14 × 14 within a 44-point hit area. Off: a 1-point `muted` outline. On: filled `brand` with a `bg`-coloured check. The label sits to its trailing side in `body`.

### Segmented control

A row of text options, `control` type, 18 apart. Unselected: `muted`. Selected: `ink`, underlined with a 1-point `brand` line 5 points below the baseline. No track and no background.

### Slider

A 1-point `rule` track with a 3-point `brand` fill up to the value, and a square 8 × 8 `ink` thumb shown only on hover, focus or drag. The hit area is 44 points tall. Times or values sit at either end in `fine`, `muted`.

### List row

The title in `body`, `ink`; secondary text in `meta`, `muted`; numbers right-aligned in `meta`, `muted`. A 1-point `rule` below each row, inset to the content's leading edge. Hover: `hover` at 30 % behind the row. The playing row's title and number are `brand`. A selected row has a `surface` fill.

### Navigation row (sidebar)

The label in `body`, `muted`, lowercase; with icons, the glyph before it in the same colour. Selected: `brand`, with the 2-point leading rule. Section headings in `fine`, `ink`, with 16 points above.

### Tab bar (phone)

Flat and full-width, with a 1-point `rule` along its top and `bg` beneath, 64 points tall. Labels in `fine`, lowercase; with icons, a glyph above each. Unselected: `muted`. Selected: `brand`, the label underlined. The mini player sits directly above it as a row with its own top rule, whose first part is the playhead in a 2-point `brand` line.

### Transport (Mac)

A full-width bar, 68 points tall, with a 1-point `rule` along its top and no shadow or material. Left: the cover, square, 40 points, then the title in `meta`, `strong` and the artist and album in `fine`, `muted`. Centre: shuffle, previous, play/pause, next and repeat as icon buttons, over the slider with times at either end. Right: the format badge (`fine`, a 1-point `rule` outline, `ink`), then sleep, output and lyrics, as text buttons in the plain variant or icon buttons with icons.

## Icons

With icons (`appearance.theme_icons = true`), each navigation row, tab, transport control and action has its glyph. Every glyph is drawn the same way: a single line, `ultraLight` to `light` weight (1.2–1.4 points at 15–20 points), monochrome, in the colour of the text beside it. No fills, no hierarchical or palette colour, no badges. Glyphs never replace a label that the plain variant shows.

The Apple apps keep the SF Symbols they name today (`Icon.*`). On Android, Material Symbols Outlined at weight 200, or Lucide, with one glyph per role mapped from the same names.

## On the platforms

- **macOS and iOS:** a `KoanTheme` value in the environment carries the tokens and whether icons are drawn, and views read it rather than hard-coding a colour or font. The setting is `appearance.theme`, `"system"` (the default) or `"koan"`, and `appearance.theme_icons`.
- **Android:** the colour tokens map to a Material 3 `ColorScheme` (`bg` → `background`, `surface` → `surface`, `brand` → `primary`, `ink` → `onBackground`, `muted` → `onSurfaceVariant`, `rule` → `outlineVariant`, `bad` → `error`), the type roles to `Typography`, and `Shapes` are all zero-radius. The components above replace Material's own where they differ: outlined rather than filled buttons, the underlined segmented control, and the flat tab bar.
