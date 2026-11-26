# Changelog

## 0.5.0

- **The kit's components, as a crate.** `koan-kit-web` on crates.io holds the kit's controls as askama components wired for Datastar, and carries the stylesheet, so one version in an app's `Cargo.lock` names both the markup and the rules that draw it. Fields link their notes to their controls, buttons are busy for exactly as long as their own request runs, sheets open through `commandfor` with no script, and a page's one `Confirm` asks before a change, or shows the server's preview of it. Every expression is the kit's own; text reaches only escaped attributes. A test fails on a class the components write that the stylesheet has no rule for. See the README.
- **The storybook is generated from the components,** each beside the call that drew it, so ui.koan.rocks shows the markup an app receives. `--serve` runs it live, with Datastar.
- **New classes:** `check` (a box leading its words, inline), `pairs` (labels and values), `empty` (where a list would be), `actions` (a row of buttons), `skip` (the skip link §8 asks for), `release` (a footer's release link), `icon`, and a sheet's `header`, `section` and `footer`.
- **The font's URL carries its hash,** `geist-mono.woff2?v=…`, so it can be cached for good and a new one is fetched again. An app serves it with the crate's `font::serve` beside its stylesheet rather than copying it out of the package.
- **The icon set is the kit's,** `koan_kit_web::icon`, compiled from the kōan apps' own SVGs.
- **The licence names the font:** `MIT AND OFL-1.1`.
- **Built on askama_datastar 0.2,** whose `SecurityHeaders`, `Csp`, `Asset` and `reopen` an app gets through the kit's re-export. An app's askama_datastar must be the kit's.

## 0.4.0

- **`qr` and `codes`,** from kōan's two-step sign-in: a QR code to scan off the screen, dark on white with its quiet zone in either scheme, and a set of one-time codes to keep, such as recovery codes, selected whole so they copy together. Each was drawn by hand wherever a page needed one.

## 0.3.0

- **Renamed `@radiosilence/koan-kit-web`,** the web counterpart of koan-kit, and moved into the koan repository beside its specification. Nothing in it changed from `@radiosilence/theme` 0.2.9: replace the package name in `package.json`, the stylesheet's `@import` and the `kit-check` path. ui.koan.rocks shows every control.

## 0.2.9

- **`combo`** joins a field to the button it feeds, such as a value and the action that uses it, so the pair reads as one control rather than two that happen to sit together.

## 0.2.8

- **`primary` is filled with the accent.** It marks the main action of a page, a card or a form, and an outline did not stand out among the bordered controls around it; mcp-gateway, slsk-mcp and mariastew had each filled it themselves. Actions repeated on every row of a long list stay bordered.

## 0.2.7

- **kit-check reads gradient stops only in class attributes.** In a code string, words such as `from-cookie` and `to-do` were taken for `from-*` and `to-*` classes.

## 0.2.6

- **A panel is a lighter ground, not an outline.** Hairline boxes on the page's own ground made a page of panels read as harsh; a panel is now `surface` with a clear border an app can tone. A code value or panel inside one drops back to `bg`.
- **`fill`**, a button filled with its tone rather than outlined: a quiet fill on the default button, the accent on `primary`, the error colour on `bad`.

## 0.2.5

- **A notice is prose.** It was a flex row, which split a sentence holding `<strong>` or a link into separate boxes; an action in it now follows the words on the same line.
- **kit-check reads class lists kept in code:** the hyphenated words of string literals in Rust and TypeScript, and `className = "…"` with spaces.

## 0.2.4

- **Labels and summaries keep their case.** They hold file names, titles and paths too often for the kit to lowercase them; a field's label is still lowercased, and an app writes its own words lowercase in the source.
- **A busy status is hollow under reduced motion,** so it still reads as unsettled when it cannot breathe.

## 0.2.3

- **A sheet clears the home indicator.** On a phone its bottom padding is at least `env(safe-area-inset-bottom)`; `--sheet-padding-bottom` sets it otherwise.
- **kit-check reads Tailwind sizes with a line-height modifier,** such as `text-xl/snug`, which compile to nothing like the bare sizes.
- **The README says the CSP needs `img-src data:`** for the checkbox's tick and the select's chevron.

## 0.2.2

- **A panel ending in a paragraph keeps even padding.** The paragraph's bottom margin had been added to the panel's padding.

## 0.2.1

- **An unchecked checkbox or radio is outlined again.** 0.2.0 drew it with no border, so it could not be seen.

## 0.2.0

- **The web kit.** Plain elements come out in the theme (the bordered button, the square field and toggle, the slider, the dialog, headings and focus), and classes give the rest: the button weights, `field`, `toggle`, `panel`, `notice`, `status`, `meter`, `busy`, `skeleton`, `code`, `rows`, `dense` tables, `disclosure`, `toast` and `sheet`. Taken from koan's web UI and the controls dither, mariastew and slsk-mcp had each written for themselves.
- **Tailwind's palette, radii, shadows, blurs and type sizes are removed.** `rounded-lg`, `shadow-xl`, `text-sm` and `bg-red-500` compile to nothing; use the theme's roles. Black and white stay, for QR codes.
- **`ok`**, a status colour that is the theme's mint in every app, whatever its accent.
- **`kit-check`**, which reports classes outside the look and inline styles, for an app's CI.
- **A storybook** of every control in every state, `mise run storybook`.
- **`tokens.css`**, the tokens alone, for a static site with markup of its own.

## 0.1.0

- **First release.** koan's tokens, type scale and Geist Mono, taken from `site/src/theme.css` in radiosilence/koan, with the `warn` colour slsk-mcp added and a light-mode value for it.
