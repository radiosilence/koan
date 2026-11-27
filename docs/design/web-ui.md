# The web UI

koan-server's web UI (`crates/koan-server/src/ui.rs` and `src/ui/`) is a
server-rendered application: askama templates and the components of
[koan-kit-web](koan-theme.md), patched into the page over server-sent events by
[Datastar](https://data-star.dev), with
[askama_datastar](https://github.com/radiosilence/askama-datastar) as the glue.
It is built the way radiosilence's other server-rendered apps are (slsk-mcp's
`ARCHITECTURE.md` describes the shared shape); this document is koan's.

The one thing koan adds to that shape is a player. Music plays in the browser,
from `/ui/stream/{id}`, and the queue lives there too, kept in local storage.
A page that loaded whole would stop the music, so every page loads in place
and the shell, which holds the player, never reloads.

## Layout

```
src/ui.rs                  the route table, UiState, the gate, assets, shared helpers
src/ui/paths.rs            every address: the route patterns and the functions writing them
src/ui/layout.rs           the shell: the nav, `main`, the transport bar, the track menu
src/ui/page.rs             the Page shape, Body, and serve (whole or in place)
src/ui/page/<page>.rs      one module per page, named as the nav names it
src/ui/stream.rs           the live stream, the Live shape, the seen handover
src/ui/stream/<page>.rs    one per page with live parts (Library)
src/ui/parts/              what several pages draw: track row, tile, credit, heart, toolbar, shelf
src/ui/sign_in.rs          sessions; sign_in/ holds the pages outside the shell
src/ui/oauth.rs            the OAuth authorization server's protocol, no pages
src/ui/fixture.rs          tests: a throwaway library, accounts and the router
src/ui/escaping.rs         tests: the escaping guard
templates/layout.html, templates/layout/   the document and its regions
templates/<page>/page.html                 each page's body, and the regions it sends again
templates/parts/                           the shared parts
templates/sign_in/                         the frame and the pages outside the shell
scripts/ui.ts              the player
```

Library's Rescan and Caches sections are its submodules
(`page/library/rescan.rs`, `page/library/caches.rs`); Account's API keys, app
passwords and two-step sign-in are Account's (`page/account/`).

## Routes and addresses

`ui::routes` declares every route in one place: the pages, the live stream,
the actions, and the admin's routes, which sit behind `admin_only` as well as
the gate, so a route added there is refused to everyone else by construction.
Sign-in, OAuth and the assets are merged from `sign_in::routes` and
`oauth::routes`.

Every address comes from `paths`: the route patterns as constants, and a
function per address that writes it relative to the page's `<base href>`,
since a proxy may serve the UI under a path. A test requests every address the
functions write through a router of the patterns, so a misspelt one fails
there rather than as a dead link. Scripts format no addresses: a track row
carries its audio, record and properties addresses in `data-` attributes, and
the player reads them.

## Pages

Every page implements `page::Page`: the nav entry it is shown under, what it
reads from the request (`askama_datastar::View` for an address's query, which
reads leniently, or `Path` for an id), and its body, drawn for the account
signed in. A body that cannot be drawn is a `Refusal` (not found, or the
library unavailable), drawn in the page's place with its status.

`page::serve::<P>` draws a page two ways. A browser asking for the address
gets the shell around the body. The page itself asking, through a link inside
an element marked `data-kit-in-place` or a `kit::Form::get`, gets the body
morphed into `main`, the title, the nav marking the page, and `$url`, the
address under the base path. `kit::Navigation::new().follows("url")`, placed
once in the shell, pushes `$url` onto the history and loads Back and Forward in
place, so a reload, a shared link, back and forward all draw what the address
names, and none of them reloads the shell. A link that must load whole (one
leaving the UI, or Last.fm's sign-in) carries `data-whole`. Sign-out, OAuth
and the assets are whole loads.

The player's script loads a page in place from an address a row carries (Go to
Album, Properties) by raising `koan-go`, which an element of the shell answers
with `@get`.

## The queue

The queue lives in the browser, but its rows come from the server, from the
same template as every other list's (`parts/track.html`). The queue page's list
names the queue it was drawn for (`data-drawn`); whenever the player's queue
differs, the script raises `koan-queue` with the tracks' ids, and the list
posts them to `/queue/rows`, which answers with the list. A track gone from the
library draws no row, and the rest keep their places in the queue.

## The live stream

One route, `/ui/live`, draws whichever page `$url` names: the Library page's
rescan and caches for an admin (`stream::library`), and `Quiet` for every other
page, which draws nothing but holds the stream open, so the page learns at
once when its session ended. The shell opens the stream with
`askama_datastar::reopen_on(…, ["url"])`, so it is opened afresh whenever the
page moves and never tracks navigation itself.

A stream waits on the job registry's and the cover cache's signals, paced by
`askama_datastar::paced` (at most one pass every quarter second, ended after a
minute for the page to open another), and sends a region only when it differs
from what it last sent (`askama_datastar::Last`). Nothing a stream draws starts
work: a scan is started by the Rescan action, and the warm by the cover cache.

**The seen handover.** A page records what it was drawn up to as `$seen`: this
process's boot (a random id made when the UI starts) and each topic's count,
read before the body was drawn. The stream's first pass draws only the topics
that moved since, and every pass that draws the page's regions sets `$seen`
beside them in the same answer, since Datastar drops a stream without telling
the server when the page is hidden or the network fails, and the replacement
reads `$seen` from the page. A `$seen` from another boot is drawn whole, since
the counts start again at zero. A pass that could not read what it draws
withholds `$seen` and ends the stream, so the one reopened draws what it
missed; a pass that drew nothing of the page's own leaves `$seen` alone.

## Actions

An action answers with as little as it can: nothing, when the page's stream
draws the change (Rescan, Warm now); the region it changed, when nothing
streams it (a heart, the account's keys, a user's row); a kit `Flash` for a
line of success or failure; `kit::Confirm::close` when it answered the page's
question. A form whose fields should empty once it is used is answered with
its region replaced rather than morphed, since a morph keeps what was typed in
inputs it matches (the keys and app passwords, the form making an account).

An action reads its input strictly: a form with `axum::Form`, signals with
`ui::Sent`, both of which refuse a body that does not parse. Only a view reads
leniently.

## Markup

Every view is a struct deriving `Template` with a file under `templates/`. A
view embeds another as `Html`, which askama writes as markup, and escapes every
other value. Nothing builds markup with `format!`, nothing calls `Html::trusted`
outside the kit, and no template writes `|safe`. Every control, status, notice,
flash, empty state, disclosure, QR code and confirmation is a kit component;
templates add layout utilities only. The few layouts several pages share are
classes of koan's own stylesheet (`tiles`, `hero`, `tracks`).

Datastar expressions are constants. A per-row value goes in an escaped `data-*`
attribute that the expression reads through `el.dataset`. Signals are named by
the program; the server reads `url`, `seen` and `forget`, so none starts with
`_`.

## Patch targets

Ids exist only on the regions the server sends again, each made by one
constant the template and its handler share:

| Where | Ids |
|---|---|
| Shell | `title`, `nav`, `main`, the kit's `flash` and `kit-confirm` |
| Album, Artist | `share` |
| Queue | `queue` |
| History | `history` |
| Library | `rescan`, `caches` |
| Account | `two-step`, `keys`, `app-passwords` |
| Users | `user-new`, `user-panel`, `users` |
| Scrobbling | `listenbrainz`, `lastfm` |
| Search | `search`, the query field, kept by its id while the page is drawn around it |

A repeated control that changes everywhere at once, a heart, is found by its
`data-fav` key, and the answer replaces every copy.

## The escaping guard

`ui::escaping` fills every field a person or a remote server controls (tags,
playlist names, account and key names) with markup-shaped text and requests
every page whole and in place, every page's live stream, the answers of the
actions that draw what was typed, and the pages outside the shell, through the
router, signed in. `askama_datastar::escaping::Guard`, told the templates' and
the kit's element names, finds markup of the templates' own escaped, text
escaped twice, or text left as markup; the test also fails on an id written
twice in one answer. A new page, stream or field a person controls belongs in
it.
