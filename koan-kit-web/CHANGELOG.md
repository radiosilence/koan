# Changelog

## 0.8.1

### Fixed

- **`Navigation` is the one owner of the history.** An in-place `Link` or `Button` on a page with a `Navigation` hands its click to it rather than pushing an entry of its own, so Back after such a load no longer leaves one page's address over another's content. Nothing is pushed until an answer has set the followed signal to the address it drew, and only once the load has settled, so a load that fails leaves the address and the page as they were; a Back or Forward load that fails replaces its entry with the address the page shows. `Navigation` now always follows a signal, `url` unless `follows` names another, which every page placing it declares; the mode that pushed the link's address before the answer is gone.
- **A request's busy signal is named by the request alone:** method, URL, form, the `named` name and value, group and id. Rows posting one URL with `.named("id", row)` are busy apart, and a label that changes while the request runs (a count, a playing mark) keeps its signal. A `Form` is named by its method, action and id, since its fields are not known as its tag is drawn: forms posting one action row by row, told apart only by a hidden field, give each form an `id` (which `Form::field` builds on) or a `busy_key`.
- **An app's classes are escaped once.** `Button`, `Link`, `Navigation` and a bare `Check` escaped them twice, so `[&>svg]:size-4` reached the page as `[&amp;>svg]:size-4`.
- **A loop over `Vec<Markup>` hands each to a slot,** through `From<&&Markup>` for `Markup` and `Content`, as the README promises of every prop.
- **A loading field's skeleton is inert,** so a custom group's controls can be neither reached nor sent before its settled self arrives.
- **A QR code draws each row's runs as one rectangle,** about a fifth of the markup it drew per module.

## 0.8.0

### Breaking

- **Markup cannot be made from a string.** `Markup` and `Content` no longer convert from `askama_datastar::Html`, since `Html::trusted` makes one of any string. A view's markup reaches a slot as a component, a `wrap` block, or `kit::Markup::render(&template)`; several pieces collect into one. `Confirm::preview` takes the same. Every element the kit sends is drawn by one of its templates: `Confirm::error`, `Disclosure::fill` and `Spinner` were written by hand, and `Confirm::go` is now `askama_datastar::go`.
- **A request's busy signal is named by what the control is.** A button's, an in-place link's and a form's own signal is a hash of its method, URL, form, group, id and label rather than a per-thread serial, so the same fragment drawn twice is byte for byte the same, and a live stream redrawing a region while its request runs leaves the spinner turning. Two controls alike in all of these on one page share a signal, which a patch adding or removing either would clear mid-request; give each an `id`. A `Form` without a `busy_key` is named by its method, action and id.
- **`Link::in_place`'s reload on Back stands down** while the page has a `Navigation`, which loads the entry in place.

### Added

- **`Navigation`, for a page that loads in place.** Placed once in the shell, it loads every same-origin link inside an element marked `data-kit-in-place` in place on a plain click, with one listener for the page, so a grid of thousands of links carries no listener each. A modified or middle click, a `target`, a `download`, a `data-whole` link and a link to a fragment of the page are the browser's. Back and Forward load the entry in place rather than reloading, so a page holding a player keeps playing. `follows(signal)` keeps the address bar on the signal the answer sets, in place of `askama_datastar::history`.
- **`Tabs` and `Tab`.** A row of addresses over a hairline, the one shown marked `aria-current="page"`, each a link that can load in place, with a count and actions (pin, close) after it.
- **`Form::field` and `Field::scope`.** A field placed through its form builds its label's and note's ids on the form's id, so a form repeated per row needs an id on the form alone.
- **`Field::loads(url)`.** A field whose control the server takes a moment to fill stands as a skeleton holding the ensō, disabled, and fetches its settled self as the page starts.
- **A failed request is said over the page.** `Flash::region().failed(words)` raises an error flash when a request's answer is an error, whose body Datastar drops; `unreachable(words)` when Datastar has given up retrying; `sign_in(path)` sends a 401 to the sign-in page.
- **`Cover::with(key)`.** A region faded, inert and under the ensō while any request of a busy group runs, such as a folder listing being left.
- **`Link::to(href)`, a card.** A link holding the markup of a `wrap` block, such as a cover over a title, drawn in the text's colour; it loads in place like any link.
- **`Qr`.** A QR code drawn by the kit from the code's modules, so an app splices no SVG of its own.
- **A `Disclosure` with an id keeps its open state across a morph,** so a live stream sending it again leaves it as the reader left it.

### Changed

- **askama_datastar 0.3.2.**
- **`Disclosure::fill` escapes its id as a CSS identifier** in the selector it builds, so an id holding a quote, a comma or a space cannot retarget the patch.
- **The storybook's examples are templates,** each shown as the file that drew it.

## 0.7.0

- **askama_datastar 0.3.** The kit's markup is askama_datastar 0.3's `Html`, so an app on 0.3 hands its own rendered markup to the kit's slots with one `Html` type. 0.3 refuses a `reopen` address holding control or space characters, which a browser would strip into another host.

## 0.6.0

- **The loader is the ensō everywhere.** `Spinner` draws it alone, at the text's size, `large`, or `over` the middle of a region such as a buffering video, and with a `label` it is a status a screen reader announces. A busy status's mark is the ensō turning in its tone, a disclosure loading its body shows it before the placeholder, a link loading in place carries it, and a skeleton holds it rather than breathing. Under reduced motion it stands whole and still.
- **A compact size.** `.compact()` on `Input`, `Select`, `Textarea`, `Field`, `Combo`, `Button` and `CopyField`, or a `compact` class on any container, draws controls in `meta` type with tight padding, so a compact field, combo and button in one row are the same height. `controls` is a row of controls stretched to the tallest, so a bordered button beside a full-size field matches it.
- **A form with several actions.** `Button::post(url).sends(form_id)` makes a button that submits a form's fields to its own URL, busy by itself. It is that form's submit, so Enter presses the first one and the form's constraints are checked first.
- **A form that asks first.** `.sends(form_id).confirm(question, yes)` checks the form, then asks in the page's `Confirm`, whose yes posts the form's fields, so Enter and `required` keep working.
- **A busy group disables, and only the pressed button spins.** Buttons sharing a `busy_key` are all disabled while any of them runs, and only the one pressed shows the spinner. `Status::busy_with(key)` turns while the group runs.
- **Links that load in place.** `Link` draws a link in the page's look, and `.in_place()`, on it or on a `Button` with `href`, reads the page with `@get` on a plain click while a modified click, a middle click or a page without script follows the real `href`. The address follows it in a history entry of its own, and Back to such an entry reloads it. It is busy while it loads and ignores a second press, and a `busy_key` makes a set of links load one page at a time. `Link::current` marks the page shown.
- **Confirm.** `Confirm::go(path)` sends the browser elsewhere once an action has removed the page it was asked on. The button that asked hears `kit-confirmed` (`kit::CONFIRMED`) once its handler closes the sheet with `Confirm::close`. `Confirm::error` takes markup as well as words.
- **Flash.** `Flash::link(href, label)` puts a link after the words and `Flash::stays()` keeps an ok flash until it is dismissed. The page's own script raises one with a `kit-flash` event on `window` (`Flash::EVENT`), whose detail is set as text.
- **Choices.** `Check::bare()` draws the box alone, named by its label, with the common props on the `<input>`, so `form=` and Datastar attributes reach it. `Check::picture(src)`, `picture_icon(glyph)` and `detail(text)` make a row of a list to choose from. A `Check` or `Toggle` takes its words from a `wrap` block, which may hold a field.
- **Content.** A `Disclosure` wrapping a block that opens with its own `<summary>` takes that as its summary. `Pairs::optional` drops a row with no value, and `Pairs` takes rows a template builds with loops from a `wrap` block. `CopyField` takes a `name`, gives its input an id from its own, and has a glyph form, `icon_only`.
- **Fields.** `Field::custom` is a group named by its label and described by its note, for several controls a `<label>` cannot hold.
- **Buttons.** `Button::toggles(glyph, pressed)` shows a second glyph while pressed, such as play and pause, and says whether it is pressed to a screen reader. A compact `icon_only` button is as tall as a compact field.
- **Glyphs:** `pin`, `pin-filled`, `muted`, `full-screen`, `leave-full-screen` and `picture-in-picture`.

## 0.5.0

- **The kit's components, as a crate.** `koan-kit-web` on crates.io holds the kit's controls as askama components wired for Datastar, and carries the stylesheet, so one version in an app's `Cargo.lock` names both the markup and the rules that draw it. Fields link their notes to their controls, buttons are busy for exactly as long as their own request runs, sheets open through `commandfor` with no script, and a page's one `Confirm` asks before a change, or shows the server's preview of it. Every expression is the kit's own; text reaches only escaped attributes. A test fails on a class the components write that the stylesheet has no rule for. See the README.
- **The storybook is generated from the components,** each beside the call that drew it, so ui.koan.rocks shows the markup an app receives. `--serve` runs it live, with Datastar.
- **New classes:** `check` (a box leading its words, inline), `pairs` (labels and values), `empty` (where a list would be), `actions` (a row of buttons), `skip` (the skip link §8 asks for), `release` (a footer's release link), `icon`, and a sheet's `header`, `section` and `footer`.
- **The font's URL carries its hash,** `geist-mono.woff2?v=…`, so it can be cached for good and a new one is fetched again. An app serves it with the crate's `font::serve` beside its stylesheet rather than copying it out of the package.
- **The icon set is the kit's,** `koan_kit_web::icon`, compiled from the kōan apps' own SVGs.
- **The spinner is the ensō,** drawing itself as koan-kit's does: a 1.6 s turn, head then tail, the same on a busy button, `.spinner` and Confirm's preview, and standing whole under reduced motion. It is a CSS mask, so the CSP needs nothing beyond `img-src data:`.
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
