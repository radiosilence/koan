# Changelog

## 0.11.2

### Added

- **`Tabs::scrolls`, a row of tabs that scrolls sideways** rather than wrapping, for a page's navigation of more tabs than fit: never vertically, with no scrollbar drawn and the current tab's underline whole. Apps scrolled their own, and the underline's overlap made the row a pixel taller than its box, so it moved vertically when dragged.
- **`--control-height` and `--control-height-compact`, the height of every control** at the two sizes, the border included.

### Changed

- **Fields, selects and buttons share one height at each size**, 40px and 28px, and centre on the line, so any row of controls lines up. Each set its height from its own type and padding: a primary button stood taller than the select beside it, and a bordered one shorter than a field. A bordered or quiet button is now taller than it was, and a primary one shorter; a link keeps its line's height, and a button whose label wraps grows to hold it.

## 0.11.1

### Added

- **`kit::rate`, a rate in the words a transfer uses** (`1.3 MB/s`), beside `kit::size`.

### Changed

- **The transfers' mover is installed once per page, by `kit::server_clock()` on the `<body>`.** Each `Transfer` carried the whole script in its own `data-init`, so a region of twenty transfers sent some 30 KB of it with every patch; each now carries its reading and a call to start it. A page holding a transfer needs `server_clock()` on its `<body>`, without which transfers show the server's reading, still. `server_clock()` writes the installer as `data-init__kit`, leaving the body's own `data-init` free.

### Fixed

- **A transfer with an unknown total draws no bytes.** It wrote "0 B" beside the waiting meter, which read as a finished download of nothing, and a magnet's metadata as progress; the bytes are drawn once the total is known.

## 0.11.0

### Added

- **`Transfer`, bytes on their way that move between readings.** Given the bytes done, the total, the rate and when the reading was taken, it draws a meter and a line of bytes, rate and time left, and the page's one mover advances them from the rate four times a second. It stops at the total, at a rate of nought, and two report intervals after the reading arrived (`every` when the server reads less often than once a second), so a stalled transfer stops rather than running on. A reading is timed from when the page received it, so a reader whose clock is wrong still sees the right rate, and the same reading renders the same markup, so a stream that sends only what changed sends it only on a new reading. Apps drew this with scripts of their own.
- **`kit::size`, a size in the words a transfer uses:** steps of 1024, one decimal place.

## 0.10.1

### Added

- **`italic()`, a common prop on every component.** Its words, and what they hold, in italic: set apart from those around them without a louder tone, such as a tab whose results have gone stale, which apps drew with a class of their own.
- **`kit::server_clock()`, the server's clock for a page's `<body>`,** which counters are read against, learned once per page.
- **`SelectionBar::action_when` and `Selection::all_have`, for an action that applies to only some rows:** shown while a ticked row carries its tag (`data-kit-tags` on the box or its row), or while "all" is ticked and every match includes some.

### Fixed

- **A counter renders the same markup for the same instant.** `Time::elapsed` and `remaining` wrote the render time and a reading worked out from it, so every render of a region holding one differed and a stream that sends only what changed re-sent it each pass. A counter now carries only its instant, and the page's one timer reads it against the clock `server_clock` gives once per page. `as_of` gives the words it says before it starts and should be a stable value; without it they are the instant in UTC.
- **A selection counts only its own boxes.** One nested inside another, or a box naming it with `form=`, was counted by the wrong one, and "select all" ticked the inner rows. It now acts on its form's own elements and its own counts and bars.
- **A selection recounts after a morph.** A patch that reset a box without adding or removing one left the count, the bar and the validity stale, so an action could send rows no longer ticked. It recounts after every patch, and folds recounts into one a frame, looking only at changes that add or remove a box, so a long streamed list is not counted whole on every patch.

### Changed

- **`Tab::tone` takes an `Option<Tone>`,** so `None` is no tone, and `Tone::Wait` draws a tab muted as it draws a status, rather than standing for no tone.

## 0.10.0

### Added

- **`Time::elapsed` and `Time::remaining`, counters that step each second.** The time spent on a stage or left until a deadline, read against the server's clock (`data-now`, the render time unless `as_of` gives the reading's), so a reader whose clock is wrong still sees the right count. One timer steps every counter on the page and stops once none is moving.
- **`Selection`, `SelectionCount` and `SelectionBar`, for rows to tick and act on together.** A form whose `select_all` box, counts and bar of actions follow its boxes as they change and as rows arrive. The bar sticks to the window's foot, clear of a phone's home indicator, and shows only while anything is ticked; with `none`, a request sending the form with nothing ticked says so instead of sending.
- **`Field::clearable`, a box that sends a field as cleared** rather than left unchanged, as a batch edit's fields need: while it is ticked the field's other controls are disabled, so only the box is sent.
- **`submits()` on every control and on `Field`,** which sends its form when its value changes, as a filter's select or date does.
- **`Input::short` and `Field::short`, a number field as wide as a few digits,** such as a limit or a port.
- **`Tab::busy` and `Tab::tone`.** A tab whose page is still working shows the ensō and is busy to a screen reader; a tone draws its words as failed, needing attention or done.
- **`Wordmark`, the app's name as its mark outside the top bar,** for a page such as sign-in that has no bar. `.wordmark` is styled wherever it stands.
- **`TopBar::in_place` and `Wordmark::in_place`,** so the link home loads in place like the app's other links.
- **`scrolls_into_view()`, the attribute that brings a region into view as the server draws it,** such as a preview drawn in answer to a request lower on the page.
- **`Pairs::tight` and `rows tight`, for a dense listing.**

### Changed

- **`Time::millis` and `Time::seconds` take their instant by value or by reference,** as the kit's other props do, so a template holding `&&i64` from a loop or a field builds a time itself.
- **A textarea resizes vertically only,** since one dragged wider breaks the layout around it.
- **`Sheet::loads` gives up after a second attempt** when the server cannot be reached, rather than staying busy for the three minutes Datastar's retries take, and closes with the unreachable flash.

## 0.9.1

### Fixed

- **A `Time` keeps its localised reading when a stream sends its region again.** A morph keeps the `<time>` but sets its text back to the server's UTC reading and does not start it again, so every older row in a feed or log read in UTC. One localiser per page, installed by the first time to start, now watches the document and rewrites a time whose text or instant a morph changed. Relative times share its one minute timer and formatters made once, where each carried a timer of its own and built a formatter on every tick.
- **A sheet that loads hides its actions until its body arrives, and closes if the read fails.** `Sheet::loads` left the footer's actions live over the ensō, so a press, or Tab and Return, could act on a body not yet there, and a read answered with an error left the ensō turning. The actions are now hidden, out of Tab's reach as well, until `Sheet::fill` replaces the body; a read that fails or cannot be sent closes the sheet, and a `Flash::region().failed(…)` says that it did not work. Apps that hid the actions or closed the sheet themselves can stop.
- **The failure flash no longer fires for streams.** `Flash::region().failed(…)` and `unreachable(…)` heard every request, so a stream refused during a deploy or reconnecting as a phone woke said "That did not work" while the person had pressed nothing, and said it again at each reopening. A request from an element `reopen_on` marks now raises no flash; a 401 from one still goes to the sign-in page.

## 0.9.0

### Breaking

- **`Tone` has a fifth variant, `Neutral`,** so a `match` over it needs an arm for it.

### Added

- **`Time`, an instant in the reader's zone.** Drawn as a `<time>` with an ISO 8601 `datetime` and the UTC reading as its text, then rewritten by the browser in the reader's zone and language: date and time, `date`, `clock` (to the second, 24-hour, for a log line) or `relative` ("3 days ago"), which is worked out against the reader's clock and recomputed each minute. The server needs no knowledge of the reader's zone, and an app no expression of its own.
- **`List`, a bulleted list for values someone else chose.** Each value behind a square marker and wrapped anywhere, so a crafted value cannot pass as two and an unbroken one cannot run off the page; `strong` for the values the reader must not miss, `empty` for what it says when there are none.
- **`.prose`, for rendered Markdown:** headings, lists, quotes, code, rules and tables in the kit's scale.
- **`RepeatField`, a field of any number of one value.** Rows of one input sent under one name, which the kit adds, removes and fills from a paste holding several values (split on lines, or on spaces with `split_words`), with an optional button per row that reads the clipboard into it.
- **`CopyField::description`.** What the value is for and where to paste it, above the field, which it describes through `aria-describedby`.
- **`Status::neutral`, for a state that is settled but not verified,** drawn in the ink: neither `ok`, which would claim it works, nor `wait`, which reads as still checking.
- **`TopBar` and `Footer`, the frame of an app's pages.** The bar holds the wordmark leading home and the app's links and actions at the far end, stays at the top and clears a phone's notch; the footer clears its home indicator.
- **`Sheet::loads(url)`, a sheet whose body the server draws as it opens.** Each opening empties it, shows the ensō and fetches; the handler answers with `Sheet::fill(id, body)`, and an action done with it with `Sheet::dismiss(id)`, which closes it.

### Fixed

- **A flash no longer strips the region's failure handlers.** `Flash::region().failed(…).unreachable(…).sign_in(…)` listened on `#flash`, which every `Flash::ok` or `Flash::bad` answer replaces, so after the first flash a failed request, a lost connection or a 401 said nothing. The handlers now listen from `#flash-failures`, a hidden element beside the region that no flash touches; an app that redrew them on every flash can stop.

### Changed

- **Every sheet keeps above an on-screen keyboard by itself.** `Sheet` and `Confirm` publish `--keyboard-inset` from `visualViewport`, which the stylesheet already read, so an app no longer measures it.

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
