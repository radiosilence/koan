# koan-kit-web

The kōan web kit: the look radiosilence's server-rendered pages are drawn in, as a stylesheet and as askama components wired for Datastar. It is to the web what `koan-kit` is to GPUI: the look and the markup live here, so an app cannot drift from either. The specification, with the reason for each rule, is [docs/design/koan-theme.md](../docs/design/koan-theme.md). Every component in every state is drawn at [ui.koan.rocks](https://ui.koan.rocks), each beside the call that drew it.

It ships as two packages at one version:

- **The `koan-kit-web` crate:** the components, carrying the stylesheet in its source.
- **The `@radiosilence/koan-kit-web` npm package:** the stylesheet alone, for a page with no Rust behind it.

## Use, from Rust

```toml
[dependencies]
koan-kit-web = "0.5"
```

The crate builds on `askama_datastar` and re-exports it, so an app's askama and askama_datastar are the kit's: `HtmlSafe` belongs to one askama release, and the components write the attribute syntax of the Datastar that crate serves.

### The stylesheet, from the crate

An app compiles its stylesheet from the kit in the crate's source, so the version in `Cargo.lock` names both the markup and the rules that draw it:

```just
# Compile the stylesheet with the kōan web kit from koan-kit-web's source, the version Cargo.lock names, so the markup
# the components write and the rules that draw it come from one release.
css:
    #!/usr/bin/env bash
    set -euo pipefail
    manifest=$(cargo metadata --format-version 1 --locked | python3 -c 'import json, sys; print(next(p["manifest_path"] for p in json.load(sys.stdin)["packages"] if p["name"] == "koan-kit-web"))')
    ln -sfn "$(dirname "$manifest")" styles/kit
    tailwindcss --input styles/app.css --output assets/app.css --minify
    node styles/kit/kit-check.mjs templates src
```

```css
@import "tailwindcss" source(none);
@source "../templates";
@source "../src";
@import "./kit/theme.css";

/* The app's accent, in both schemes, each at least 4.5:1 on --surface. */
:root {
  --brand: #4fd6f0;
}

@media (prefers-color-scheme: light) {
  :root {
    --brand: #066f82;
  }
}
```

`styles/kit` is a symlink `just css` makes, so it is gitignored; the compiled stylesheet is committed, as §8 asks, so `cargo build` needs neither Node nor Tailwind. The components use the kit's own classes and no utilities, so the app's Tailwind needs to scan only the app's templates.

### The page

askama resolves `{% extends %}` against the app's own templates, so an app keeps its base template and places the kit's parts in it:

```html
<!doctype html>
<html lang="en-GB">
<head>
  {{ kit::Head::new(self.css()).title(self.title()).datastar("/assets/datastar.js") }}
</head>
<body>
  {{ kit::SkipLink::new("skip to content", "main") }}
  <main id="main">{% block main %}{% endblock %}</main>
  <footer>{{ kit::Release::new("slsk", crate::VERSION, "https://github.com/radiosilence/slsk-mcp") }}</footer>
  {{ kit::Confirm::new("cancel") }}
  {{ kit::Flash::region() }}
</body>
</html>
```

Its view's module has `use koan_kit_web as kit;`. `Head` writes the charset, the viewport, both colour schemes with the browser's chrome in the kit's ground, the font's preload, the stylesheet, an import map and Datastar's script. The app routes the font beside its stylesheet, where the stylesheet asks for it:

```rust
.route("/assets/app.css", get(stylesheet))
.route("/assets/geist-mono.woff2", get(koan_kit_web::font::serve))
.route("/assets/datastar.js", get(askama_datastar::script::serve))
```

The Content Security Policy needs `img-src 'self' data:` for the checkbox's tick and the select's chevron, `script-src 'self' 'unsafe-eval'` for Datastar, and an import map's `ImportMap::csp_source()` when the page has one.

## Components

A component is a value: built with its props, then rendered wherever askama renders a value, inline in a template or as a field of a view.

```html
{{ kit::Field::email("email", "email").required().hint("For sign-in only.").error(error) }}
{{ kit::Status::ok("connected") }}
{{ kit::Button::quiet("remove").bad().post(url).confirm("remove this album?", "remove") }}
{{ kit::icon::HEART.svg(15) }}
```

Every prop takes what a view holds, by value or by reference. askama hands a template's variables over by reference (`&String`, `&&String` from a loop, `&u64`, `&bool`, `&Option<String>`) and its literals and calls by value, and each prop converts from all of them, so a template never writes `*count` or `&name`. A prop that may be left out reads empty text as absent, so an `Option<String>` field passes as it is: `.error(error)`.

Text is escaped wherever it lands. Markup is a separate type, built from an `askama_datastar::Html`, a component or a filter block's content, and no string converts into it, so a person's text cannot be passed where markup goes.

| Component | What it draws |
|---|---|
| `Button` | Every action. Its weight is its constructor (`primary`, `bordered`, `standard`, `quiet`, `link`); `bad`, `fill`, `icon`, `icon_only`, `busy`, `disabled` qualify it; one action says what it does: `post`, `get`, `href`, `opens`, `closes`, `submit` |
| `Form` | A form's attributes for the app's own `<form>` (`post`, `get`, `plain`, `live`), and its submit |
| `Field` | A label, a control and a note under it, linked by `aria-describedby`: `text`, `password`, `email`, `url`, `number`, `select`, `textarea`, `custom`; `hint`, `error`, `warning` |
| `Input`, `Select`, `Textarea` | The controls, alone or in a field |
| `Combo` | A control joined to the button it feeds |
| `Check`, `Toggle` | A box leading its words inline; a settings row with the box trailing. Each a checkbox or a radio |
| `Status`, `Meter` | A state in a tone, in words; progress as a hairline, named for a screen reader |
| `Notice`, `Flash`, `Empty` | A message in place; one over the page; what stands where a list would be |
| `Sheet`, `Confirm` | A dialog opened through `commandfor`; the page's one sheet for asking before a change |
| `Pairs`, `Disclosure`, `CopyField` | Labels and values; a summary that opens or loads; a value to copy |
| `Head`, `ImportMap`, `SkipLink`, `Release` | The page around the content |
| `icon::*` | The kōan icon set, `kit::icon::HEART.svg(15)` |

The common props, on every component: `id`, `class` for the app's own layout utilities, `data`, `attr`, and `datastar` for a Datastar attribute the kit does not write. Every component also has `html` and `patch`, so a handler answers with one: `events([kit::Flash::ok("Saved.").patch().into()])`.

Containers whose markup is one element and a class stay classes: `panel`, `rows`, `dense`, `code`, `codes` (one-time codes to keep, selected whole), `qr` (a QR code's `<svg>`, dark on white in either scheme), `skeleton`.

### Children

A component with a slot takes its content as a prop, or from the template that places it, through the `wrap` filter. askama renders the block, escaping its values as everywhere, and hands it over marked safe:

```html
{% filter kit::wrap(kit::Sheet::new("add", "add a download").close("close")) %}
  {% let form = kit::Form::post("/downloads") %}
  <form {{ form.attrs() }} class="grid gap-4">
    {{ kit::Field::url("link", "link").required() }}
    {{ form.submit(kit::Button::primary("add")) }}
  </form>
{% endfilter %}
```

`Sheet`, `Disclosure`, `Notice`, `Empty` and `Field::custom` have a slot.

### Datastar

Every expression a component writes is the kit's own constant. A URL, a question or an id goes in a `data-*` attribute, escaped, and the expression reads it with `el.dataset`, so nothing a person typed reaches script (§8). Names an app gives, a signal to bind or an expression through `.datastar()`, are `&'static str`: by convention the program's own text.

- **Busy while its request runs.** A control that sends a request is busy, disabled and spinning for exactly as long as its own request runs. Its signal is named for the request, a lowercase hash of the method and URL, so two controls posting the same thing share it and any other two never do.
- **Forms.** A `Form` keeps its real `method` and `action`, so a submit made before Datastar loads stays off the query string; `require_datastar_on_post` refuses that submit, which is how it should fail. A form that must work without script, such as sign-in or consent, is `Form::plain`, posting to a route outside that layer which checks the request's origin itself.
- **Asking first.** `Button::confirm(question, yes)` and `Button::preview(url, question, yes)` ask in the page's one `Confirm`, so a list of a thousand rows carries no thousand dialogs. Opening it empties what it last held and disables yes until this button's question, or this button's preview, is in place; each opening cancels the last one's preview request, so a late answer cannot land under another question. A preview's handler answers `events(kit::Confirm::preview(plan))`. The sheet stays open while yes's request runs, busy, and the action's handler closes it with `kit::Confirm::close()`, so a failure is seen where it happened.

## Use, without Rust

```css
@import "tailwindcss" source(none);
@import "@radiosilence/koan-kit-web";
```

Plain elements come out in the theme, and the classes the components use give the rest: the button weights, `field`, `combo`, `check`, `toggle`, `panel`, `notice`, `status`, `meter`, `pairs`, `empty`, `qr`, `codes`, `sheet` and its parts, `actions`, `skip`. A static site with markup of its own imports `@radiosilence/koan-kit-web/tokens.css` for the tokens alone. The package is published to GitHub Packages, so installing it needs a token with `read:packages`.

Tailwind's palette, radii, shadows, blurs and type sizes are removed: `rounded-lg`, `shadow-xl`, `text-sm` and `bg-red-500` compile to nothing. What that cannot reach, such as `rounded-full` and arbitrary values, kit-check reports.

## kit-check

```sh
node kit-check.mjs templates src
```

Reports classes outside the look in templates and in code that writes markup: rounded corners, shadows, rings, blur, gradients, Tailwind's palette and type sizes, arbitrary colours, and `style=` attributes, which the CSP blocks. A deliberate exception carries `kit: raw` and its reason on the line or the line above.

## The storybook

`storybook/index.html` is generated from the components, so ui.koan.rocks shows the markup apps receive:

```sh
cargo run -p koan-kit-web --example storybook --features storybook              # writes storybook/index.html
mise run storybook                                                              # compiles its stylesheet
cargo run -p koan-kit-web --example storybook --features storybook -- --serve   # serves it live, with Datastar
```

The written page is static, since a static site ships no Datastar; served, the same page talks to stub handlers, so a busy button, a question, a preview and a live search can be tried. A test fails when the written page is stale, and another renders every component and fails on a class `kit.css` has no rule for.

## Adding to the kit

A look an app needs and the kit lacks is added here, not in the app, once a second app needs it or it is a control the specification describes. A component that writes markup two apps repeat is added to the crate the same way. Each gets a place in the storybook in each of its states.

## Release

The crate and the npm package carry one version, in `Cargo.toml` and `package.json`, which a test and the workflow check. A push to main that changes it publishes both, from `.github/workflows/koan-kit-web.yml`. Its notes are its section of `CHANGELOG.md`; it is not tagged, since koan's own `v*` tags would collide. The kit is released on its own, not with koan; koan uses it from the tree, and its release refuses to go out while the kit has changed since its version last moved, since the published koan-server names the kit by version.

Until 0.2.9 it was `@radiosilence/theme`, in radiosilence/theme, which keeps its history.

## Licence

MIT (`LICENSE`), with Geist Mono under the SIL Open Font License (`OFL.txt`).
