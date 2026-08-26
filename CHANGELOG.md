# Changelog

## Unreleased

### Added

- **A graphics level, in Settings -> Appearance.** One slider from `Plain` to `Full`, so koan can be told how much of the machine it may spend on looking like itself. `Full` is what it has always done and stays the default.

  | | Wash | Indicators | Chrome |
  |---|---|---|---|
  | `Full` | drifts | dance | glass |
  | `Reduced` | held still | dance | glass |
  | `Plain` | none | held still | flat materials |

  The steps are ordered by what they were measured to cost rather than by how much they look like they cost. On an M1 Pro, playing, window frontmost: `Full` is 15-18% of a core, `Reduced` around 9%, `Plain` around 6%.

  `Reduced` is where it is because of what the wash's cost actually is. The blur is rasterised once at 360 points and magnified as a texture, so it is close to free -- what is expensive is the *drift*: three animations of incommensurate period running forever on scale, rotation and offset, which is the only thing making the wash's composited output differ frame to frame, so the copy mirrored out under the sidebar and toolbar and the glass sampling it are redone for as long as the music runs. Held still, the wash measures the same as no wash at all. The record's colour is free; only the breathing is billed.

  Below that, `Plain` is for machines where drawing a blurred backdrop is dear at all. Most of what it saves over `Reduced` is the playing indicators standing still, which stops a 30fps timeline and the analyser poll behind it. It also swaps the glass chrome for flat materials, which measured about a point of a core on top -- close enough to the noise floor that it is there on the reasoning rather than on the evidence, for GPUs and display sizes unlike this one.

  The setting lives in macOS defaults (`defaults write cc.blit.koan graphics -int 0`) rather than `config.toml`: how much this app draws is this machine's business, and the TUI has none of it to draw.

### Fixed

- **The wash's drift is far enough to see.** It has moved by the same amounts since it landed in #330, and those amounts were too small to register: blurred at 14 points and magnified about five times, the wash has no feature on screen narrower than eighty points, and the drift carried it about one and a half of those over twenty-three seconds. Slow movement that small does not read as movement -- it reads as a still image. The thing was running, and costing six to nine percent of a core to run, and there was nothing to see.

  The excursions now reach about four of those instead: 12% of the window sideways against 4%, 10% down against 6%, and the scale swinging 1.38 to 1.58 rather than 1.19 to 1.31. The periods are untouched at 13, 19 and 23 seconds, so it is exactly as unhurried as it was -- it simply arrives somewhere.

  The wider scale is not decoration. It is the floor that keeps the texture's own edge out of frame once the offset carries it 12% of the window sideways and the rotation eats another three and a half percent. The scale is also taken off the longer edge of the window now rather than its width: the texture is square, so on a window taller than it is wide the width alone never covered it.

- **The wash honours Reduce Motion.** It never did. The playing indicators already went still when the system asked for less motion and the room behind them kept breathing.

### Changed

- **The macOS app queries the library instead of copying it.** It used to load every album and every artist at launch, narrow those copies in Swift and index them so search could resolve ids against them -- three shapes of the same five thousand rows, held to serve views that read none of them directly. Now a section asks the engine what it should be showing and shows exactly that; narrowing and sorting happen in SQL.

  Nothing is paged. This is an in-process call rather than a wire, so a listing arrives whole: the scrollbar tells the truth about how long the library is, and one flick reaches the end of it.

  The bugs this closes are the ones that came from the copy existing: a section showing a library the database no longer has, and a cold launch showing an empty one because the load lived somewhere the second window never reached. There is no load to have forgotten to do.

  `AlbumSort::Random` now takes a seed, so narrowing a shuffled listing narrows the shuffle you are looking at instead of dealing a new one on every keystroke. A new seed is a new shuffle, which is what the reshuffle button asks for.

- **The engine says when the library changed.** A new `LibraryChanged` event rides the same channel as `QueueChanged` and `DownloadsChanged`, raised by anything that writes library rows -- scan, sync, import, organize, forget, rebuild -- including the automatic sync and the watched-folder scan that nothing was announcing at all. A background scan finishing now reaches the browser the same way one you asked for does, and the app no longer refreshes itself by guessing from whatever it happened to start.

- **`koan-core` narrows and orders albums and artists itself.** `list_albums` and `list_artists` take a search term, an order, a favourites-only flag and an optional limit, replacing the several near-identical queries that answered one shape of the question each. Play history and favourite tracks take a search term too, and fuzzy album and artist search hands back rows rather than ids for a caller to resolve.

## v0.31.2 (2026-08-25)

### Changed

- **Credentials moved out of the OS keychain and into `config.local.toml`. Anyone signed in to a remote server will have to sign in once more.**

  A keychain item's ACL is keyed on the code signature of the binary reading it, and koan has no stable signing identity — ad-hoc signing derives that identity from the binary's own hash, so every release was a different application to macOS. No grant ever matched twice, "Always Allow" was a promise about a binary that was about to stop existing, and the login-password dialog came back on the first launch after every update. The apps that never ask are not doing anything clever: they are Developer ID signed, so their identity survives an update and the ACL keeps matching.

  What the dialog was guarding does not justify it. Subsonic authenticates every request with the password or a salted MD5 of it, so a client has to keep something password-equivalent indefinitely — there is no token to exchange it for, and Navidrome offers no OAuth. The secret now sits in `config.local.toml`, gitignored and created `0600`, beside the URL and username it belongs to. That is the bargain `~/.netrc`, `~/.aws/credentials` and `gh`'s `hosts.yml` all make, and it costs a keychain that could never be made to hold.

  The remote password, the Subsonic API secret and the refresh token from `koan auth login` all move. `koan auth login` writes a new `[auth]` section naming the server it signed in to. The `keyring` dependency and `KOAN_NO_KEYCHAIN` are gone, along with the test-suite opt-out that only existed because unsigned test binaries could never match an ACL either.

### Added

- **The app says so when it has a server configured and no password for it.** Nothing did. The queue filled with tracks that never loaded, every sleeve came back empty and no download started — which reads as a broken library rather than as being signed out, and sends you looking in the wrong place for it. A toast at launch now names the server and points at Settings, which is where the one thing that fixes it lives.

- **A button that puts the queue back on the row that is playing.** Beside the layout picker, since both are about what you are looking at rather than what is in the queue. The row is centred rather than dropped at the top edge — what is playing is read against what comes after it. It runs the same path `g` and `G` do, and is disabled rather than hidden when nothing is playing.

### Fixed

- **The queue comes back to where you left it.** A trip to an album and back dropped you at the top of it again. The stage builds one page at a time, so leaving the queue destroyed it — and a macOS `List` cannot be put back: its scroll position belongs to AppKit's table, and every SwiftUI way of asking for one (`scrollPosition`, `scrollTo(y:)`, `scrollPosition(id:)`) is quietly inert on it. The queue is no longer torn down. It stays where it is, behind whatever you navigated to: invisible, untouchable, and off stage, which is also what stops the bars on the playing row dancing to an analyser nobody can see. The selection you left survives the trip too.

### Removed

- **The macOS app is Apple silicon only.** It shipped as a universal binary, and the Intel slice was over half the release build: two cross compilations of the engine, lipo'd together, then a universal Swift build on top. That is a long time and a lot of a runner's disk — enough that the v0.31.1 release ran out of it partway through writing the disk image — to serve machines whose newest supported macOS is the app's minimum.

  koan itself is unchanged on Intel: `koan` the terminal player still builds and ships for `x86_64-apple-darwin`, and an Intel Mac that wants koan can run that. It is the SwiftUI app, and only the app, that now needs Apple silicon.

## v0.31.1 (2026-08-25)

### Changed

- **Every page takes the record's colour, not just the ones about a record.** The wash was drawn on the queue, on an album page and on a playlist, and nowhere else — so the albums grid, the artist list, an artist's page, favourites, history and search results were flat grey rooms you passed through on the way back to a coloured one. Each of those pages was opaque precisely to hide the wash the window was already drawing behind it.

  They give that ground up. A page about one record still answers with it — an album with its own sleeve, a playlist with the first of its records — and every other page answers with what is playing. The room is coloured by the music wherever you have wandered off to, and only a page that disagrees says otherwise.

  The wash and the control tint were two properties computing nearly the same answer, and now they are one: what the room is washed in and what the buttons are tinted with can no longer disagree about which record you are looking at. Nothing new is drawn — there is still exactly one `ArtworkBleed`, on the window, which is what `backgroundExtensionEffect` mirrors out under the sidebar and toolbar. The pages simply stop covering it up.

- **Artist chips are a plain fill rather than glass.** Glass samples what is behind it and adapts its own luminance to stay legible against it — right for something floating over content, wrong for a chip sitting in it. On a flat page ground every pill sampled the same colour and they all matched; over the wash they each answered to a different part of it, so a row of them read as a scatter of half-transparent ones rather than a set. A fixed fill takes its share of the colour behind it without arguing with it.

## v0.31.0 (2026-08-25)

### Added

- **Playlists.** Named, ordered lists of tracks that outlive the session — the thing the queue could never be, and the thing saved queues were standing in for. They live in the sidebar under their own heading, with a 2×2 mosaic of the first four records in them for a face.

  Almost everything the queue does applies: album headings over contiguous runs, multi-select, drag to reorder, ⌫ to remove, drop to add. A playlist opens ungrouped rather than grouped, because a playlist is a sequence someone chose rather than a shelf of records — and that choice is remembered per playlist, on this machine, since no server has anywhere to put it.

  Anything playable goes in one: a track, a record, an artist, a selection, the whole queue. Drag onto a playlist's row to append; drag onto the *heading* to be asked for a name and get a new playlist of what you dropped. Double-click one, or press Play in its header, and it replaces the queue. Playback state is mirrored back onto it the way it is on an album page. There is a **Shuffle** that reorders the queue and a **Shuffle Playlist** that reorders the playlist; they are different verbs and they are both there.

  They can be exported as extended M3U8. Only tracks with a file on this machine go in — a Subsonic stream URL carries the credentials that authorise it, and a playlist file is something people mail to each other — so the export says how many it had to leave out.

- **Dropping into a playlist lands where you dropped it,** with a line showing where that is. Adding used to mean adding to the end, wherever the pointer was.

- **Playlists sync with Navidrome, both ways.** A playlist made here appears on the server; one made there appears here, contents and order included. Every edit pushes in the background, so nothing waits on the network; a push that never got out leaves the local copy newer, and the next sync notices and sends it. Deleting on either side deletes on the other, because a playlist that comes back after you delete it cannot be deleted at all.

  Only what Subsonic itself carries is stored — name, comment, owner, public, and the ordered song list — so the two sides are the same object rather than koan's idea of one. Where a playlist sits in your sidebar, and how you like to look at it, are the exceptions: those are facts about this machine.

- **Subsonic playlist endpoints serve real playlists.** `getPlaylists`, `getPlaylist`, `createPlaylist` and `deletePlaylist` used to fake playlists out of saved queues; they now read and write the real thing, and `updatePlaylist` — add and remove members by index — works at last.

- **The playing indicator moves to the music.** The three bars beside the current row rode a pair of sine waves, which said "playing" truthfully but said it the same way through a drum break and a held piano chord. They now follow the analyser: low, mid and high, one band per bar.

  The audio modulates the waves rather than driving them. A band never reaches a height directly — only how far its bar is already travelling — so a chorus makes the bars swell and a quiet passage settles them low and slow, and no transient can make them spike. They never come fully to rest while the transport runs, because the first thing the indicator has to say is which row is playing. Each band is judged against its own recent loudest, so a track mastered quiet gets the same indicator as a loud one. A track still buffering, or the first frames after a start, run the plain carrier.

  One poller feeds every indicator on screen and stops when there are none, or when nothing is playing. Reduce Motion keeps the still bars.

- **`q` goes to the queue.** `g` and `G` already went to its ends; there was no key for simply going there.

- **Escape clears the selection, wherever you are.** The queue also grew a Clear button beside Remove — a selection with no visible way out of it is a trap, and on a list you have scrolled away from you cannot even see what you are still holding.

- **The TUI organize modal honours `[organize] default`.** The macOS sheet already preselected the pattern it names; the TUI took whichever sorted first.

- **Favourites shows records and artists, not only tracks.** koan has always let you favourite an album or an artist — the heart is on both — and the Favourites page only ever listed tracks, so there was nowhere in the app those went. It is one page in three sections now, the way search results are: a section only appears when you have favourited something of that kind, so a tracks-only library reads exactly as it did. The filter narrows all three at once.

- **The seek bar shows how much of a streaming track has arrived.** Playing something off a remote library while it is still downloading, the bar showed a position on a full-width track: whether the rest of it was on the machine, or the transfer was limping and playback was about to stall, was not visible anywhere near the thing that would stall. The fetched extent is now drawn behind the played one, and retires when the track lands.

  It is a fraction of bytes on an axis of time — right for lossless and CBR, out by however far the bitrate wanders on VBR — so it is drawn as a distinctly weaker mark than the played extent and the tooltip says "roughly". The question it answers is whether the music is about to run out of track, not where in the track anything is; a hard-edged fill would be read as a promise it cannot keep. The TUI has drawn the same three-way bar all along.

### Changed

- **The queue stays locked to the playlist that filled it.** Play a playlist and the queue *is* that playlist; reorder it, remove from it or add to it and the queue follows, quietly. Rearrange the queue yourself, add to it, or let radio extend it and the two part company — from then on the playlist is a document you are editing and the queue is what you are listening to.

  Locked is derived rather than tracked: queue items carry the playlist row they came from, so the queue is locked exactly when its entries are that playlist's, in that order. Nothing to keep in sync, nothing to persist, and it cannot get stuck — a queue that stops matching stops being locked, and one that matches again is locked again. Playing a playlist shuffled scrambles the order on purpose, so that queue is not locked, which is the right answer rather than a special case.

  Records lock too, and need no provenance to do it: an album *is* an ordered set of tracks, so the queue being that album is a question about what the queue holds — which means it still answers for a queue restored from a previous session, where nothing remembers what was played. A record cannot be edited, so there is nothing to follow; it just says what you are listening to.

  The queue says so: while it is locked it is headed **Playing *<name>***, with the playlist's mosaic or the record's sleeve beside it, and goes back to plain **Queue** the moment it is not. A rule this quiet has to be visible, or the first silent update reads as koan moving things on its own.


- **The shortcuts sheet says what the menus say.** It listed the single-key shortcuts and then told you the rest were "in the menus", which is true and no help — nobody opens six menus to find out that Back is ⌘[. The ⌘ shortcuts are now a second table on the same sheet, and both halves are generated from the same declarations the menu bar is built from, so they cannot drift.

- **Leaving the queue and coming back keeps your place.** The stage is one `switch`, so every move destroys the page it left and takes its scroll position with it. The queue remembers where it was.

- **Playing something no longer throws you at the queue.** Every play button replaced the queue and then moved you to it, which is backwards: the queue runs behind whatever you are doing and is somewhere you go, not somewhere you are sent. Play a track from a record, a row in favourites, a selection in history, an album from the picker — you stay where you were and the music changes.

  Two places still move you, because staying put would show you nothing. An artist page is a wall of covers with no tracks on it, so its play and shuffle buttons go to the queue. And a click on an album's cover art opens the record it just started, since that is where its tracks are.

- **The database schema is now version 2, and older koan builds will refuse to open it.** That is the point of the version: a build that does not know about `playlists` must not write to a library that has them. Upgrade rather than downgrading.

- **Settings are routed to the file they belong in.** `config.toml` is meant to be shareable, so three kinds of setting never land there: secrets, anything naming this machine's paths, hardware or account, and volatile UI state a keypress flips — `playback.art_size` and the four visualiser toggles that have keybinds. Everything else is taste and travels. Three call sites previously disagreed about this: the player wrote `output_device` to the shared file, the GraphQL settings mutation wrote `art_size` and the remote account there too, and the FFI wrote everything to the local one. `config::layer_of` is now the single answer, and `koan config init` builds its template from it, so the file it writes and the files koan writes cannot drift.

- **A write clears the same key from the other file.** `config.local.toml` wins the merge, so a shared write left shadowed by a local copy silently did nothing. In the other direction it drains machine-scoped keys out of `config.toml`, which cleans up a file an older koan polluted as you use the app.

- **`[graphql] subsonic_port` is now `[subsonic] port`.** The port for the Subsonic API lived in the GraphQL section, so turning Subsonic on meant a key in each of two sections. `[subsonic] enabled` mounts `/rest/*` on the GraphQL port; `port` adds a dedicated listener.

- **`[discovery]` is gone.** It held one setting koan read and one it did not; `analysis_on_scan` moves to `[library] analyze_on_scan`, where the rest of the scan options are.

- **The menus have their icons back.** Every action in the app now carries the same symbol wherever it appears: Add to Queue is the same icon on an album page, in the row's context menu and in the menu bar, and the Edit menu — replaced wholesale so ⌘Z can reach koan's own undo stack rather than a text field's — draws its own icons again instead of arriving bare. The symbols are named in one place, which is what stops the three copies of a verb drifting apart.

### Removed

- **Snapshots.** They were playlists with a resume position: a whole second feature, a second table, a second sidebar row and a second set of API calls, all to remember one number the server had no idea about. Existing snapshots become playlists on first launch, keeping their names, their track order and the date they were saved; only the position is lost. The Subsonic API already served them as playlists, so its clients see the same names either side of this.

  ⌘6 is gone with them, and **Save as Snapshot…** in the queue menu is now **Save as Playlist…**.

- **The download quality setting.** It offered Original, Opus 128 and MP3 320, wrote your choice to `config.toml`, carried it over GraphQL and FFI, and was read by nothing — no request has ever asked a server to transcode. Re-encoding a stream is the opposite of what koan is for, so the setting goes rather than gains an implementation. `remote.transcode_quality` in an existing config is ignored; the GraphQL `Config.transcodeQuality` field and its input are gone.

- **`[radio] use_subsonic` and `[discovery] acoustic_weight` never did anything.** Both were documented as tuning knobs and neither was read: the acoustic signal's weight was a constant in the scoring function, and nothing consulted `use_subsonic` before deciding whether to ask a server — see below for what radio actually does. Removed rather than wired up — the defaults are the behaviour everyone has been getting.

- **`[playback] ticker_fps`.** The transport ticker scrolls at a fixed 8 characters per second.

Unknown keys are ignored rather than rejected, so a stale config still loads. Two
renames change behaviour silently and are worth grepping your config for:
`[graphql] subsonic_port` (now `[subsonic] port`) and `[discovery]
analysis_on_scan` (now `[library] analyze_on_scan`). The others were inert.

### Fixed

- **The playing mark crowded the sleeve beside it.** Six points pairs a status mark with a right-aligned track number, which carries slack of its own; a cover is a solid block flush to its frame and the same six points read as a squeeze. A list that draws sleeves gives the mark the same ten points the title gets on the other side.

- **The queue got slower the more of it you had played.** A played row was the whole row at 45% alpha, and any alpha below 1 makes SwiftUI flatten that row into an offscreen layer before compositing it — its children overlap, so they cannot each be drawn at 45% and still look like one row. A long tail of played tracks behind the cursor was a long list of offscreen passes, on every frame the list drew. Played rows now step back in colour instead: a rung down the hierarchical styles, which costs a colour lookup. Only the sleeve still uses alpha, because no colour can dim an image — and one 34pt image is not a flattened row.

- **Most of the ways koan syncs never touched playlists, and koan's own auto-sync never touched favourites either.** Four callers each knew separately what a sync consists of — the app, the CLI, the GraphQL job and the background auto-sync — so a star or a playlist made on the server arrived only if you happened to press the right button. There is one `sync_remote` now and all four go through it.

- **The queue quietly dropped repeats.** Adding a track already in the queue added nothing, and a playlist holding the same song twice played it once. The batch fetch behind every queue add took each row out of its map as it matched it, so an id asked for twice came back once.

- **koan wrote your whole configuration to the file you commit.** Every setting koan saved itself — a visualiser toggle, the output device, dragging the album art wider — reserialised the entire `Config` struct over `config.toml`. That erased the commented-out reference `koan config init` writes, and filled the file people are told to put in a dotfiles repo with all fifty-odd keys, including `[library] folders` set to koan's *default* music directory and a `[remote]` block belonging to one machine's account. Writes now go through `Config::persist`, which diffs the mutation and touches only the keys that actually changed.

- **`koan config init` deleted the patterns you wrote by hand.** It is documented as safe to run on an existing setup, but it regenerated the template from koan's default serialisation — and `[organize] default` and `[organize.patterns]` are held back by `skip_serializing_if`, so they never appeared in it and were dropped. It also emitted `[organize]` twice when carrying values across, which is not valid TOML, and resurrected settings koan had retired. It now re-comments any value that matches the default too, so a `config.toml` an older koan filled with its own serialisation shrinks back to a template.

- **`koan remote login` wrote your password to disk.** The macOS sign-in path already stored it in the platform credential store and explicitly cleared any plaintext copy; the CLI wrote it straight into `config.local.toml`. Both go through `helpers::set_remote_credentials` now. koan's own Subsonic secret also read the config field *before* the keychain, so a stale plaintext copy beat the real secret — the credential store wins in both, and the config field stays what it always was: the fallback for machines without one.

- **`[radio] seed_window` was half honoured.** It picked the seed artists, while the acoustic seeder asked for the last five tracks regardless. One window, one answer.

- **The radio documentation described a feature that does not run.** The README and the radio guide both led with Subsonic `getSimilarSongs2` and a cache of similar-artist relationships as radio's strongest signals. Neither is reached: the auto-queue loop turns the network signals off before it picks, because ListenBrainz and MusicBrainz rate-limit to one request a second per seed artist and all three are synchronous HTTP in front of a track that is needed before the current one ends. What radio actually scores is genre and era, same-artist, acoustic similarity over bliss-audio vectors, and a low-scored random tail — all database reads.

  The code is unchanged; the docs now say what it does, and the guide says why the network signals are switched off and what would have to happen to switch them back on. Two things follow from the same gap and are now written down: nothing writes the `similar_artists` cache, so `similarArtists` over GraphQL and FFI and `getSimilarSongs2` on koan's own Subsonic API return empty; and radio behaving identically offline is not graceful degradation, because there is no online path to degrade from.

  `koan scan --analyze` is the one thing that meaningfully improves radio, so the docs now say that instead of burying it fifth in a list.

- **`remote.transcode_quality` was still documented after being removed.** It survived in the remote-servers guide — with a table of quality tiers and a note that "Navidrome and most Subsonic servers handle this natively" — and in the configuration reference. The setting was never wired to anything and is gone; so is the section describing it.

- **Documented settings that were not true.** `bass_shake` defaults to `true`, not `false`. `[organize] default` preselects a pattern in the organize UI, not "the TUI modal" it had no effect on. The format-string reference showed a `koan organize --pattern` command that has never existed — organize runs from the TUI and the macOS sheet.

- **koan grew to a gigabyte of artwork and never gave any of it back.** Every cover you scrolled past was decoded and held for the life of the process — nothing evicted, so an hour of browsing a remote library reached 978 MB of decoded bitmaps, most of it swapped out rather than released. Covers are now held in a bounded cache that hands memory back under pressure.

  Bitmaps are also kept at the size they are drawn rather than the size they arrived. A queue row shows a sleeve at 28 points; it was holding the full 600-pixel image to do it, one per row. Rows and the transport keep a thumbnail, the grid and a record's own page keep a tile, and full size is decoded on demand for the artwork viewer and released with it — which is the only place the detail was ever visible.

- **The queue raced to fetch the same sleeve once per track.** Queue rows asked for artwork by track, so a twelve-track album was twelve HTTP round trips, twelve files on disk and twelve identical bitmaps in memory for one image. `QueueItem` now carries the album it came off — resolved for the whole queue in one query — and every row on a record shares a single fetch.

  This also closes a hole in the placeholder detection. Navidrome answers with a stock blue vinyl for anything with no artwork, and koan spots it by noticing the same image on three unrelated albums; lookups by track never took part in that vote, so the queue and the transport would draw the placeholder the grid had already learned to hide.

- **The window wash cost a quarter of a core to sit still.** It was drawn twice — once on the window, once on the page on top of it — and the page's copy sat on an opaque ground hiding the window's, which kept animating behind it for no one. There is one now, on the window, which is the one that mirrors out under the sidebar and toolbar.

  The one that remains no longer redraws on a timer. It moved its blur through scale, rotation and offset twenty times a second, and each tick invalidated layout: a full window Auto Layout pass, 20 Hz, whether or not anything had changed. The drift is handed to CoreAnimation instead, which runs it off the main thread and smoother for it. Stopping playback now settles the wash back to rest over a couple of seconds rather than freezing it mid-breath.

- **The same timer cost a third of a core everywhere else it was used.** The three bars marking the playing row drove their *height* from a `TimelineView`, and the seek bar sized its played portion and its fetched extent with `frame(width:)`. Height and width are layout, so each of those forty-odd ticks a second marked the window dirty and AppKit answered with a full Auto Layout pass over the whole view tree — a third of a core to move nine points of bar and one line, and it went on doing it while the window was minimised, because a `TimelineView` does not care whether anyone is looking. Both are drawn in a `Canvas` now: same bars, same marks, same motion, on a box whose geometry never changes. Playing with the window minimised went from 28% of a core to 15%.

- **The wash re-blurred the cover on every frame it drifted.** `.blur` and `.saturation` sat above the drift in the modifier chain, so SwiftUI recomputed a blurred, saturated copy of the sleeve sixty-odd times a second for as long as anything was playing. The transforms were never the expensive part — re-deriving what they moved was. The blurred wash is rasterised once now and the drift magnifies that; switching the motion off entirely reached 17% of a core, keeping it and rasterising once reaches 13%, from 21–25% before. Nothing about how the wash looks or moves changes.

- **The playing bars ran at twice the rate of the numbers behind them.** The indicator's timeline was pinned to 1/60 while the analyser it reads only produces new levels at 1/30, so every other frame redrew data that had not moved. In this window a frame is not free — each one is a SwiftUI graph update, and each of those costs a whole-window Auto Layout pass. Both now read the same constant.

- **The spectrum analyser ran with nothing reading it.** `Player::spawn` starts it unconditionally, so a client with no visualiser on screen still paid for a 2048-point FFT and a fresh waveform allocation sixty times a second and dropped every frame on the floor. It now stands down a second after the last reader of its snapshot goes away, and picks back up within 250 ms of one returning — which is what the playing indicators already ask for and release as they come and go.

- **The filter on the Favourites page did nothing.** `LibraryModel` narrowed the list into `visibleFavourites` and the page rendered the unfiltered `favourites` beside it. Every other filtered section read the right one.

- **Ungrouped queue rows had no room for their covers.** Every row carries its own sleeve when the queue is not grouped by album, and the fixed row height around it left the art all but touching the separators above and below. It gets the same six points the album heading already gives its own cover — they are rows in the same list and should breathe alike.

- **The favourite key flipped the database and nothing else.** `f` and ⌘D went straight to the engine, but every heart in the app reads `LibraryModel.favouriteTrackIds` — so the row changed underneath a UI that kept showing the old answer, and pressing the heart afterwards looked like it was undoing a favourite you had just added. Both routes go through the library now, which is what the hearts read.

- **Undoing a queue replace left the player on a track the queue no longer had.** The queue came back, which is the part you could see working, but the engine kept decoding whatever the replace had started — an item the restored queue does not contain. The transport then described a row nothing could select, and the decode lookahead finds the next track by locating the current one, so with the current one gone the queue ended at the end of that track rather than carrying on.

  Playback is reconciled with the playlist after every undo and redo, not just this one: any undo that takes items away can strand the engine the same way — undoing an add while the added track is playing did it too. If something was playing, the restored cursor picks up where the queue says it was; if it was paused or stopped, the undo stays quiet, because an undo is not a reason to start the music. The position is not part of what a replace snapshots, so the track starts again rather than resuming mid-way.

- **The format badge latched a rate the device was on its way off.** A 48 kHz track followed by a 44.1 kHz one publishes the new track's info before the device has finished reclocking, and a switch is not instant — the better part of a second on USB. A front end polling in that window paired 44.1 with the outgoing 48, and then never let go: the macOS app only republished the format when the codec, source rate or bit depth changed, and none of those move once the rate lands. "FLAC 16/44.1 → 48" could sit there, wrong, for half an hour. The output rate now reads as unknown while the device is between rates, rather than as the last track's.

- **The format badge could claim bit-perfect while the HAL was resampling.** The output rate behind "FLAC 16/44.1 → 48" was read once, when the engine was built, and never again. But the device rate belongs to no one app: Audio MIDI Setup, Focusrite Control, anything else holding the device can move it a second later, and koan then resamples to reach it without noticing — or, the other way round, goes on asserting a match that no longer holds. Since koan does not take hog mode, losing the rate is expected; reporting it wrongly is not. The rate is now watched for as long as the engine lives, and both front ends follow it. The macOS app also had to be told that the output rate is a reason to redraw the badge — it only republished the format when the codec, source rate or bit depth changed, none of which move when another app steals the clock.

- **Every Opus track started 40 ms late.** The bridge between symphonia's demuxer and `opus-decoder` dropped its first two packets, on the reasoning that an Ogg stream opens with `OpusHead` and `OpusTags`. Symphonia consumes both into `extra_data` before we ever see a packet — it is where the bridge reads the channel count and pre-skip from — so the two it threw away were music, and Matroska and WebM never carry those headers as packets at all. Header packets are recognised by their magic now, which costs nothing to a reader that strips them and is correct for one that doesn't.

- **A bad Opus packet could take the decode thread down with it.** `opus-decoder` 0.1.1 overflows a shift building CELT's collapse mask on the first packet of some stereo streams: a panic in a debug build, a wrong mask in a release one. It is the only Opus decoder on crates.io that isn't libopus over FFI, and it is unmaintained, so the decode call is contained — that packet is skipped and logged, and playback carries on, which is already how a malformed packet is handled.

- **The README said Opus wasn't supported.** It was removed from the format list on the grounds that symphonia ships no Opus decoder. That much is true and always has been, which is why koan has bridged `opus-decoder` since v0.20.3.

## v0.30.2 (2026-08-25)

### Added

- **A heart on the transport.** ⌘D has always favourited what is playing from anywhere, but a heart you can see also tells you whether this one is already in. It sits next to the title, because that is what it acts on.

### Fixed

- **Favourites rows lost their covers again.** They were given their own sleeve and album name in v0.30.0, on the grounds that a list gathered from the whole library has nothing above it saying what each row is. Rebuilding the detail column in v0.30.1 reproduced the call that draws it without the flag that turns the mode on.

- **A flash of bare colour on the first keystroke of a search.** The page's ground was put behind it before it was told to fill the column, and a background sizes itself to what it backs — so the ground was only ever as big as the page's content. The results page measures nothing while the query is still running, which left the window's wash showing everywhere around it. The page is filled first now, and every page carries an opaque ground of its own with the wash drawn over it rather than through it.

- **The Now Playing widget had no cover on it.** Control Center, the lock screen and the media-key HUD showed the title and the artist against a blank square. Now Playing was only ever handed the artwork if the cover happened to already be in the cache at the instant the track started — and it never is: fetching it is an HTTP round trip on a remote library, and it lands a moment later. By then the guard that stops a 10 Hz poll republishing unchanged metadata saw the same track, the same state and no seek, and returned. The art arriving is now a reason to republish, and Now Playing asks for the cover itself rather than hoping the transport bar's request has landed, so it works with the window closed.

- **The cached count sat still while an album downloaded.** "N of M remote cached" in the sidebar was read once at launch and again after a scan or a sync, and a download is neither — the count moved in the database and nothing told the library to look again. A finished transfer refreshes it now.

- **Download progress juddered, and everything else slowed down while it ran.** Progress was announced by rewriting the queue item's load state after every 64KB — which took the playlist write lock and bumped the queue version, a thousand times a transfer, five transfers at once. Every front end reads that version as "the queue changed", so the macOS app was refetching the whole queue across the FFI ten to twenty times a second for what is a byte counter.

  The load state says *that* a download is running and hands out the counter; the counter is where progress lives, and the download thread writes it without taking a lock at all. It is announced once per attempt. Progress reaches the macOS app as its own event instead of as a queue change, so it moves at 10 Hz without anything being rebuilt — and the last few tracks of an album no longer freeze at whatever fraction they had reached, which is what happened when nothing was left *waiting* to download and the old nudge stopped firing.

### Changed

- **The format badge says when the output is being resampled.** koan switches the device to the source rate, and when a device refuses — MPEG-2 and MPEG-2.5 MP3 rates routinely are — the system resamples to reach it. The player has always known which of those happened, because `set_device_sample_rate` returns the rate the device settled at, and it compared the two and wrote a line to the log. Nothing else ever saw it. Meanwhile the badge showed the source format either way, captioned "koan matches the device rate rather than resampling" — an unconditional claim that is false in exactly the case worth knowing about.

  The settled rate now reaches the front ends: the macOS badge appends it — `FLAC 24/96 → 48` — the TUI's format line does the same, and `nowPlaying.track.outputSampleRate` carries it over GraphQL. What it does not do is claim bit-perfection. koan never takes the device exclusively, so another application's audio can be mixed in and the volume stage may scale in software, and none of that is visible from inside the process. What koan can say for certain is whether it handed the device the samples as they are, or something had to resample to reach it — so that is all it says.

## v0.30.1 (2026-08-25)

### Changed

- **⌘K is the search.** It is the search everywhere it exists, and koan's knows albums, artists and tracks. It opened the sheet that builds a queue instead, which is a different job — that keeps its place in the menu and moves to ⇧⌘K.

### Fixed

- **Picking something from the search dropdown landed you on the list instead.** Choosing an album sent you to the album grid, an artist to the artist list. The detail column was a `NavigationStack` whose root switched per section, and a stack throws its path away when its root changes under it, then writes the empty path back — so a move that set a section and a destination at once was undone in the update that made it. Search was the only thing that did both, which is why nothing else broke.

  The stack is gone. koan navigates like a browser and always did: any page reachable from any page, with a linear history and a cursor, which `Navigator` already owned — Back and Forward moved that cursor, and the stack's own back button was hidden so they could. It was navigating nothing and charging a hierarchy for it. An album is reachable from the queue, from the grid, from an artist and from search, and none of those is its parent. Now there is one page, one list of pages visited, and one cursor into it; the sidebar lights a row when the page *is* that row, and nothing when you are on a record.

- **Navigating anywhere could take two seconds.** Opening an album from the queue and then clicking through to its artist slid the page in as though it were being dragged. `.animation(_:value:)` animates *every* animatable change in the subtree it is attached to, not only the value it names, and it was attached to the whole split view to cross-fade the tint between records — so any navigation that happened to coincide with a new colour was stretched to the length of that cross-fade. The tint is animated where it is set instead, which is the only thing that was ever meant to move.

## v0.30.0 (2026-08-24)

### Changed

- **The window takes its colour from the record.** koan's chrome used to be pink whatever was playing. Now the cover of the record on — or of the album page you opened — is blurred out into a wash across the whole window, under the toolbar, the sidebar and the transport, so every piece of glass in the app sits in that colour. It drifts while something is playing, on three periods that never line up so it never reads as a loop, and settles where it is when you stop. One record dissolves into the next over two seconds; the old cover stays up until the new one is in hand rather than wiping through a placeholder, and a record with no art is no wash rather than a grey one.

  The controls follow it. The declared accent becomes a near-black neutral — all it drives is what AppKit draws, list selection and focus rings, and at that weight it stops competing — while the SwiftUI tint is the dominant colour of the sleeve: a saturation-weighted circular mean of hue, clamped to stay legible on dark chrome. An average would not do, because averaging every pixel of a busy cover gives the same brown-grey every time as opposite hues cancel.

  It follows the *record*, not the track: artwork is fetched per track, so re-deriving it would ask the server for another copy of the same sleeve every few minutes. And it is not tied to scroll position — the wash says what is playing, and following the scroll would have it announce whichever record you happened to be looking at.

- **The macOS app is built out of Liquid Glass.** Not a coat of blur over the old chrome — the real macOS 26 material, and the layout changes it asks for. The transport is a slab of glass floating over the stage rather than a bar welded to the window's bottom edge behind a divider: the queue scrolls *under* it, fading out at the soft scroll edge as it goes, because glass with nothing moving behind it is just a grey rectangle.

  It hangs off the window rather than the detail column. A `NavigationStack` drops decoration applied around it the moment it pushes, which took the transport off every album and artist page; each screen now makes its own room for it instead. It stops short of the sidebar, and takes the full width when the sidebar is closed. Play/pause is bigger than the two beside it and no longer goes dead when nothing is queued — it is the control you reach for without looking.

- **The toolbar says what belongs together.** `ToolbarSpacer` replaces a `Spacer` smuggled inside a `ToolbarItem`, so filtering and sorting sit on separate panes of glass instead of sharing one joined capsule with the lyrics toggle. Its background is hidden, so the wash runs behind it rather than stopping in a grey line.

- **Everything laid over artwork is glass now.** The codec badge on a sleeve is clear glass rather than a black scrim hiding the corner it sits on; the favourite heart gets a ground of its own instead of a drop shadow fighting whatever is behind it, and grows in on hover. Artist chips, the shortcut sheet's key caps, the picker's commit bar and the error toast follow — the toast tinted rather than bordered, since glass already has an edge.

- **The transport and the queue show what changed.** The playing indicator's rule — motion tells you a state is live, where a glyph only names it — applied to the four places that most needed it. Play/pause morphs between its symbols rather than swapping them. A skip bounces the arrow that was pressed: on a remote library the next track takes a moment to load, and until it does nothing else on the bar has moved, so the press reads as dropped and gets repeated. The transport's title and artist cross-fade on a track change, which gapless playback otherwise leaves unmarked. Cover art fades in rather than cutting, because in the album grid covers land tens of milliseconds apart and the hard cut reads as a stutter of pops.

  A downloading queue row said the same thing twice — a static arrow in the status column and a progress bar further along it — so the bar is gone and the arrow is the ring that fills, in the column the eye already reads for state. Reduce Motion drops the bounce; the cross-fades stay, being fades rather than movement.
- **Favourites rows carry their own cover.** A record's tracklist has one sleeve above it and numbers down the side; favourites is a list gathered from the whole library, where the cover and the album name are what tell one row from the next. The shared track list learned the mode history rows already used, so both now look the same and there is one row to change.

### Fixed

- **Favourites and history can be filtered, which they were already built for.** Both narrow on title, artist and album, history has an empty state for when a filter matches nothing, and neither could be typed into: the toolbar offered its field to albums and artists by name, and ⌘F named the same two. Which sections have a filter is now one answer on `Navigator.Section` that the field and ⌘F both read, so a section cannot be filterable in the model and not on screen. Favourites also draws the narrowed list rather than the whole one, and counts what it is showing.

- **The sidebar said no remote tracks were cached, however many were.** The count asked for tracks whose `source` is `'cached'` — a value the schema allows and nothing writes, because downloading a track does not change where it came from. What a download writes is `cached_path`, which is what `set_cached_path` sets and clearing the cache nulls. Counted from there it agrees with the files on disk.
### Added

- **The artist and album in the transport bar are links.** The bar named what was playing and gave you no way to get to it — its second line was the artist as plain text, and the record it came from was not shown at all. It reads `artist — album` now, each name going where its own name says, through the same `LinkText` the rows, grid cells and queue headers use. That was the last place in the app an artist name was not a link.

## v0.29.1 (2026-08-24)

### Changed

- **The playing row is marked by bars rather than a speaker.** A speaker glyph says "sound comes out of here", which is true of the whole application; what a row needs to say is *this one, and it is still moving*. Three bars ride a pair of sine waves whose frequencies sit at an irrational ratio, so the pattern never settles into a loop the eye can catch, and they freeze where they stand when the transport pauses — paused is the absence of motion, so it costs no second glyph. Reduce Motion gets the bars at rest.

## v0.29.0 (2026-08-24)

### Added

- **The queue groups by album or gives every track its own row, and remembers which you chose.** Grouped is what it was: a heading per contiguous run of a record, tracks underneath carrying a number. Ungrouped drops the headings and gives each row its own sleeve and its full attribution, because there is no heading above it to say what record it is. Each suits a different queue — an album listen wants the headings, a long shuffled queue wants every row to identify itself — so it is a toggle in the queue bar rather than a decision made for you, and it persists.

  Both modes are shown with the active one lit, the way Finder switches view. One icon would have had to choose between naming the mode you are in and the mode you would get, and whichever it named, the other reading is available and wrong.

### Changed

- **Scanning stops reading the cover art it throws away.** `ParseOptions::read_cover_art(false)` tells lofty not to *decode* a picture frame, not to skip it: holding nothing but a `Read`, it streams the frame into `io::sink()`. Embedded art is around 95% of the average ID3v2 tag, so a 48,000-file library spent 4.1 GiB of every scan pulling JPEGs off the disk to drop them on the floor. koan now walks the frame headers itself and hands lofty a reader that answers with zeros over the picture frames and the trailing padding — both of which lofty is contractually discarding — so those bytes never leave the disk. Over 4,000 real MP3s that is 865 MB unread, 216 KiB a file, with every tag and audio property parsing byte-for-byte as before. There is a per-file floor that bytes don't touch, so expect a scan to shorten by something nearer a fifth than a third. A tag the walk cannot mirror lofty over — unsynchronised v2.2/v2.3, an extended header, a frame ID that isn't one — is read the old way.

- **The macOS app drops the AppKit workarounds its old deployment target needed.** The floor is macOS 26, so: the hand cursor over a link is `.pointerStyle(.link)` rather than pushing and popping `NSCursor` — which leaked a cursor off the stack whenever a hovered row scrolled away; the artwork sheet sizes itself from a window `RootView` measures with `.onGeometryChange` rather than an `NSViewRepresentable` reaching for `sheetParent` and setting state from inside layout; Add Folder is `.fileImporter` rather than `NSOpenPanel.runModal()` spinning a nested run loop under the main actor; and the organize window no longer stamps its own `NSWindow.identifier`, because SwiftUI now puts the scene id there itself.

  The single-key shortcuts ask for the main window by that scene id instead of naming the windows they must ignore, so a new window scene is no longer a bug waiting for someone to add it to the list. Six files stop importing AppKit; the seven that still do — the key monitor, the responder-chain edit commands, the pasteboard, `NSSearchField`, and Now Playing artwork — have no SwiftUI equivalent.

- **The queue's album headings carry the record, not a label.** The cover was 22pt — too small to recognise a sleeve by — and the heading read as one run-on line of artist, album and year. Art is 52pt, and the text is the album, its artist, then year · track count · running time · codec, so a run in the queue says what it is without counting rows. The codec only shows when the whole run shares one.

### Fixed

- **Everything but the TUI downloaded one track at a time.** `remote.download_workers` defaults to 5 and the macOS settings pane offers it, but the code path behind it ignored the value: the download queue — a worker pool, a permit-limited priority lane, and a watcher that reorders around the playback cursor — lived in the TUI crate, and `koan-ffi` cannot import `koan-tui`. What everything else got instead was a stand-in that spawned one thread per batch and walked it with a `for` loop. Queue an album from the macOS app and it fetched track after track, in submission order, ignoring where you actually were in the queue.

  The queue moves to `koan-core`, where it should have been — it imported nothing but `koan-core` already. The macOS app, the GraphQL server and radio's auto-extend all reach it through the same helper they already called, so all three now download in parallel and promote what you jump to, and several tracks report progress at once instead of one.

- **A track that can never load no longer parks the queue on itself forever.** `play()` leaves the cursor on an item that is not yet Ready and waits for `TrackReady` — and a download that fails never sends one. With the library folder offline and the remote unreadable, every item failed and the player sat stopped on the first one, which is the same picture as a queue still fetching. Failures raise `TrackFailed` now, and the cursor walks on to the next item that can still load, or stops cleanly when there is none.

- **The reason a track cannot play reaches the front ends.** It was a string inside `LoadState::Failed` that nothing read: the TUI drew `!`, the macOS app drew a triangle captioned "Couldn't be fetched", and the actual sentence — a locked keychain, a server that would not answer — lived in the log file. `QueueEntry` carries it, so the TUI raises it once per distinct reason rather than once per track, the triangle's tooltip says it, and GraphQL clients can read `QueueEntry.failureReason`.

- **Sharing a track asked the config file whether there was a password.** The same mistake `koan remote status` had in v0.27: `remote.password` is empty for every keychain-backed sign-in, so "Remote not configured" was the answer on a perfectly good setup. It goes through `subsonic_client` and reports `remote_unavailable()` like everything else.

- **Clicks that registered and did nothing.** Four separate things cleared or replaced the detail column's navigation path — a section switch, the sidebar's selection setter, a history move, and the push itself — and none knew about the others, so a push could be undone by an unrelated update landing in the same pass. `Navigator` owns it now: one `Location` value holding the section and the stack pushed on top of it, one history of locations behind it, and a single move that writes both. The sidebar's highlight is stored rather than derived from the stack, so a `List` writing its selection back can no longer pop what was just pushed, and the stack's root is a view of its own, so changing section cannot make SwiftUI discard the path against it.

  Two consequences worth knowing. Back and Forward now restore the whole location, so returning to a section you had pushed into puts you back where you were rather than at its top. And clicking the sidebar row you are already on does nothing — that write is indistinguishable from one SwiftUI makes itself, and honouring it was half the bug. Back, from the toolbar or ⌘[, is the way out of a detail view.

- **Picking a search suggestion lands in the section the thing lives in.** It used to push onto the results page and then empty the field, which left you standing on a root that no longer had anything in it. An album opens under Albums, an artist under Artists, and Back returns to whatever you were doing before you searched.

- **Correcting a file's tags re-merges it with its remote copy.** `upsert_track` matched by path first, so once a row existed for a file every later scan updated it in place and never reconsidered the cross-source merge. That is wrong exactly when the original tags were bad: a track indexed as `Golden Skans (David E Sugar R` with no track number could not content-match the copy synced from the server, and fixing the tags gave it the right title and a second identical row rather than one. A row matched by path or remote id is now asked the content-match question again against the corrected metadata, and folds its counterpart in if it is there — play history concatenates, lyrics and the embedding fill a gap, favourites follow the path. The album the bad tags invented is dropped with it.

  A library already holding duplicates from this is repaired by `koan scan --force`; a plain scan skips files whose mtime and size have not changed, and it is the re-read that spots the merge.

- **A macOS build made without Xcode had invisible controls.** The accent is read from the asset catalog, compiling it needs `actool`, and that ships with Xcode proper rather than the command line tools. Without it the colour resolved to nothing and the whole app was tinted with nothing — which does not merely lose the colour: every borderless button and the playing row's title and speaker are drawn in `.tint`, so they were invisible rather than uncoloured. It falls back to the system accent, which is visible and still visibly not koan's.

## v0.28.0 (2026-08-24)

### Added

- **Track results drag and right-click like every other row.** The album tiles and artist pills beside them already did both; the track row was a `Button`, which claims the press, so it was the one result you could only click. It takes the row behaviour the library lists use — full-width hit area, drag to enqueue — and the context menu the tiles and pills already had. Clicking still goes to the album.

### Changed

- **Filtering the album and artist lists is a query, not a pass over everything the client is holding.** The macOS app narrowed its own copy of the library with `localizedCaseInsensitiveContains`: on a 5,500-album, 7,000-artist library that is 26ms of main thread per keystroke — and it ran for every section, not the one on screen. `find_albums` joins `find_artists` in koan-core so every front end narrows the same way, the FFI's `albums()` takes a `search`, and the GraphQL resolver stops filtering a fully-loaded list in Rust. The app debounces and cancels, so holding a key down is one round trip.

  Matching is ASCII case-insensitive now, as `find_artists` already was — SQLite's `NOCASE` does not fold accented letters, so `MÖTLEY` no longer finds `Mötley`. A folded search column is the fix and is not here yet.

- **The `LIBRARY` collation folds each name once per thread rather than once per comparison.** Building a sort key means an NFD pass and a `Vec` of fresh `String`s, and a collation is asked about the same name once per level of the sort — around two dozen times in a five-thousand-row list. Sorting an album list built roughly 140,000 keys where 5,500 would do. Every client's lists load faster for it.

- **`koan-ffi` sorts albums with `sort_by_cached_key`.** `sort_by_key` recomputes its key on every comparison, so sorting by title lowercased each one a couple of dozen times over.

- **Album artwork is decoded and downsampled off the main actor.** `NSImage(data:)` defers the decode until the image is drawn, which put it on the main thread mid-scroll — and embedded artwork is routinely 1500px square for a tile shown at under 200pt. Grid tiles are downsampled to 512px; a cover opened on its own is left alone.

- **The artist list's hover state lives on the row.** It was on the browser, so every pointer move across the list invalidated all of it. The album and artist detail views also scanned the whole catalogue to find the record they were already showing; there is an index for that.

- **Engine events are an async sequence rather than a callback interface.** `PlayerEvents` was a trait the client implemented and registered; it is now `next_event()`, awaited in a loop, and `PlayerEvent` carries what changed. The loop *is* the subscription, so its lifetime is the client's task and there is nothing to register or unregister.

  The macOS app loses the object that existed only to bridge the two worlds — a callback has to be `Sendable` while the model is main-actor isolated, so every one of its three methods did nothing but hop back with `Task { @MainActor }`. It also loses an FFI round trip per event: the change already carries the new state, and the app was re-fetching it.

  A client that falls behind now drops the events it missed instead of delaying the engine. Every variant carries an absolute value — a snapshot, a version, a position — so the one that does arrive is complete on its own.

- **Drag sources outside list rows went through `.onDrag`.** It claims the press outright, which costs the tap underneath it — `RowBehaviour` had already found this and moved list rows to `.draggable`, whose recogniser has a movement threshold and leaves a press that never moves as a click. Album tiles, artist pills and artist names were still on the old path. `PlayableTransfer` is `Transferable` with the two representations the item provider was registering by hand, so drops inside koan and the plain-text fallback out of it are unchanged.

### Fixed

- **`just` ran everything with the credential store switched off.** `export KOAN_NO_KEYCHAIN := "1"` sat directly above `check:`, reading as though it belonged to it — but a top-level `export` in a justfile reaches every recipe, so `just macos-run` launched the app with no keychain. With no password there is no server, and koan degraded three ways at once: nothing played, no downloads started, and every record came back with no artwork, which reads as an empty library rather than as being signed out. It is set per-recipe now, on the two that run unsigned test binaries.

- **A client that cannot reach its server says so.** `remote_unavailable()` has produced the right sentence since v0.27 and was called in exactly one place — a `warn:` in the download queue that only a log reader would ever see. `KoanEngine::remote_problem()` asks the question directly, the macOS app asks at startup and reports through the error toast it already had, and `cover_art` returns an error when the server is configured but unusable instead of answering "no art" for every record in the library.

- **A failed artwork fetch is no longer remembered as "this record has no art".** The cache could not tell a server that said no from one that could not be reached, so a blip during a scroll left permanent holes until relaunch. Only a definite answer is recorded now.

- **Dragging an artist's name in the artists list dragged nothing.** The name was a `Button`, and a button consumes the press a drag starts from — so the row queued when dragged and the link on it, standing for the same artist, did not. The app now has one kind of link text rather than two, and every name that navigates to something playable is also a drag source for it.

- **The Homebrew cask warned on every `brew` command.** `depends_on macos: ">= :tahoe"` is a deprecated string comparison; the bare symbol has always meant "that version or newer", so the requirement is unchanged. The workflow that regenerates the cask on release is fixed too, not just the published tap.

- **The foot of the macOS sidebar was cut off by the transport bar.** The bar is a `safeAreaInset` on the split view, which reserves space in the window but not inside a column's own scroll view. The detail column already compensated; the sidebar never did, so its footer was laid out against a frame whose bottom sits behind the bar — the divider and the first activity row showed, and the progress bar, the cancel button and the library counts underneath them did not. During a sync you got a label with nothing to say how far along it was.

  The sidebar takes the same measured inset the detail column takes, so the footer grows upward from the top of the bar.

- **Clicking a search result did nothing.** Albums, artists and tracks on the results page all registered the click and stayed put; the same tiles and pills worked in the album and artist browsers, which is what made it look like a hit-testing problem rather than a navigation one.

  The sidebar's lit row was derived from the detail stack, so that an album opened from anywhere lit Albums. Opening one therefore *moved* the sidebar selection — and a `List` writes a moved selection back through its binding. That setter pops to the section root, which is right when you click a sidebar row and wrong when the List is echoing a move the app just made: it threw away the destination that had only just been pushed. The browsers were unaffected because the lit row was already the right one, so nothing moved and nothing echoed.

  The lit row is now the section being browsed and nothing else. Opening an album from search results keeps Results lit, and Back returns you to them.

- **Picking from the search dropdown landed on an empty results page instead of what you picked.** Choosing a suggestion pushes its album or artist and then empties the field, and both happened in one update: the results page is the stack's root at that moment, so clearing the query changed what that root drew while the destination was still landing, and it was discarded against the root it had been pushed onto. Emptying the field is its own update now, so the push settles first.

## v0.27.0 (2026-08-24)

### Added

- **`KOAN_CONFIG_DIR` points koan at a different configuration directory**, so one machine can run more than one library. `config_dir()` honours it, and `set_config_dir()` does the same in-process.

- **The macOS app takes the TUI's single-key shortcuts.** `space`, `<`/`>`, `,`/`.`, `f`, `R`, `p`, `/`, `l`/`a`, `r`, `g`/`G`, `L`, `z` and `?` do there what they do in the TUI, and Help ▸ Keyboard Shortcuts (`?`) lists them — generated from the table that implements them, so the two cannot drift.

  They are handled by a local event monitor rather than declared as menu shortcuts. A modifier-less key equivalent is claimed by the menu bar before the responder chain sees it, which would mean `f` favouriting a track instead of typing an f into the search field. The monitor asks what has focus first: the keys are live in the app and dead in any text field, sheet or the settings window. The cost is type-select in lists, which koan's browsers have a filter field for.

  `Esc` now leaves a search or filter field rather than only clearing it, and `⌘F` focuses the filter on the albums and artists pages, the library search everywhere else.

- **File organization in the macOS app.** Dropping a folder of files on the queue indexes them into the library where they lie and queues them; **Organize Files…** in the queue or library context menu then opens a sheet that previews where a pattern puts each one, and moves them when you say so.

  The preview is the feature. Every selected file gets a row showing its destination relative to the library folder, *including the ones that can't move* — a destination already occupied, or two files resolving to the same path, is an orange row next to the path it collided with rather than a number in an error count underneath. Nothing is ever overwritten, so the only way that matters is if you can see it before pressing the button.

  Patterns come from `[organize.patterns]`, shared with the CLI and TUI, and can be edited in place: the preview follows what you type, and saving writes it back to `config.toml` under its name. An edited pattern previews and runs without being saved, so trying one out costs nothing. With more than one library folder configured you pick which one to organize into; the CLI's behaviour (the first) is the default.

  Dropped files get library rows *where they are*, not in the music tree — importing and organizing are separate on purpose, so files land in the library only after you have seen where they are going.

  It opens in a window rather than a sheet. A sheet cannot be resized — AppKit leaves the style mask off and SwiftUI pins its content size — and a table of file paths is exactly the thing you want to make wider. A window also leaves the library visible behind it, which suits a preview you are checking against rather than a prompt you are answering.

  Whether cover art and cue sheets travel with the music is a checkbox in the sheet and a `[organize] move_ancillary` setting behind it, so the CLI and TUI organize the way the app just did.

  Generating destinations is separated from asking the disk about them, because only one of those is fast. `organize::generate` formats a pattern against an already-resolved selection and touches no files at all, so it reruns on every keystroke; `organize::check_against_disk` is the `stat`-per-file pass that finds occupied destinations and the artwork travelling alongside, and it lands a moment later. Ancillary files were previously discovered with a directory read *per file*, so an album of a dozen tracks did the same `readdir` a dozen times.

### Changed

- **The macOS app requires macOS 26.** It was built against 14. Nothing in the app is holding the old floor up and the newer SwiftUI is worth having — `searchFocused`, which is what lets `/` put the caret in the search field, is 15-and-later on its own. The Homebrew cask requires Tahoe to match.

- **`organizePreview` and `organizeExecute` both return `OrganizePlan`** (was `OrganizePreview` / `OrganizeResult`), an ordered list of per-file entries carrying `outcome` (`MOVE` / `UNCHANGED` / `CONFLICT` / `ERROR`), the destination, and the reason where there is one. **Breaking for GraphQL clients**: `moves`, `errors` and `skipped` are gone. A conflict previously arrived as a string in `errors` with the destination buried in the message, which is unusable for the thing a client most needs to render — a file about to be blocked, next to what is blocking it. The TUI's preview gains the same rows.

- **The FFI never blocks its caller.** Every one of the 80 exported calls that can block is `async` now and runs on a worker thread; the six that stay synchronous read a single atomic or an O(1) length and cannot block by construction. Previously all of them were synchronous FFI functions and the hazard lived in doc comments, which had already failed — `lyrics()` was documented as safe to call from a view body and opened a database connection, and six paths in the macOS app reached the engine directly on the main actor, one of them from inside a SwiftUI `ForEach`.

  Blocking moved into Rust rather than being wrapped at each call site. The app used to hop with `Task.detached`, which runs on Swift's cooperative pool — sized to the core count — so a cover-art storm or a scan starved every other task in the process. The engine's pool grows on demand and is deliberately not core-sized: a thread waiting on a socket costs a kernel stack, not CPU, and capping blocking work at the core count queues it behind nothing. koan-core stays synchronous, which is what its three synchronous consumers and its dedicated audio threads want; only the boundary changed.

  Queue mutations and transport commands share one lane, in submission order. They previously each got their own `Task.detached`, so two in quick succession could reach the player in either order — dropping in an album and pressing undo could undo it before it landed.

  Every `Task.detached` around an engine call is gone from the app, along with the helper that wrapped them.

- **Nothing played when the library drive was offline, and nothing said why.** The log read `remote not configured` while the remote was configured perfectly well — what koan could not do was read the password, because macOS grants keychain access per binary and the app had never been granted it. `subsonic_client` returns nothing for four different reasons and every caller reported the same one, so "koan has no password" and "koan cannot read the password it has" printed identically, despite sending you to completely different places.

  Tracks that cannot be fetched are marked failed with the reason now, instead of sitting as pending forever. The player waits for a track to become ready, so a download that was skipped left the queue silent until it ran off the end.

  `koan remote status` asks the way koan asks. It read the config field directly and never consulted the credential store, so it reported `password: not set` for every keychain-backed sign-in — the arrangement `koan remote login` creates — and then skipped its own connectivity check because of that same flag. The one tool that should have diagnosed this was reporting the opposite of the truth.

### Fixed

- **The test suite read the developer's own configuration, and asked for their keychain password.** `Config::load()` resolves its directory from `$HOME`, so tests inherited whatever library folders and remote server belonged to whoever ran them. A koan-tui *rendering* test spawned the download queue, which resolved the configured server and reached for the credentials to reach it with — and macOS put up a dialog asking for the login keychain password. The suite then blocked on it: koan-server's tests took 25.52s waiting, and 0.10s when the keychain was disabled.

  Nothing was hardcoded; that is what made it easy to miss. The tests were simply configured as the person running them, and CI never noticed because a runner has no `~/.config/koan/` to inherit. On a developer's machine the same code path would have gone on to fetch tracks from their server.

  Tests point the configuration at a disposable directory now, so they read a config nobody has edited. The ones that need a remote build their own rather than borrowing one.

- **Menu shortcuts no longer reach past a field you are typing in.** ⌘← and ⌥← skipped tracks instead of moving the caret or the word, and ⌘Z undid a queue edit instead of the typing — in the search field, a filter, Settings, anywhere. Every shortcut whose key also means something while typing is now *disabled* while a field has focus, which releases its key equivalent to the responder chain; declining the action instead would leave the menu swallowing the key, so the shortcut would stop working without the field ever hearing it. The bare-key shortcuts already asked what had focus; the modified ones never did.

- **The sync fetched 1,725 artists and threw the list away.** `get_artists` was called, counted, logged as "syncing 1725 artists" and then dropped — artists only ever came into being as a side effect of a track upsert, which saw nothing but a name and an id. That is why not one artist in a synced library had a MusicBrainz id or a sort name, and why the reported artist count was theatre. The list is applied now, and the count is what was actually written.

- **Album and track metadata the server had already sent was discarded.** An album row is reached through a track, so it only ever saw what a file's tags said; track totals, the record label and the MusicBrainz id are properties of the release and came back in the same `getAlbumList2` response the sync already pages through. `songCount` was even parsed into `SubsonicAlbum`, covered by a test, and stored nowhere. Albums and tracks gain `mbid`, albums gain `sort_name`, and `total_tracks` and `label` are filled — all from responses koan was already making, at no extra request. Enrichment fills blanks rather than overwriting, so a locally-scanned album keeps what its tags said.

- **Remote tracks carried no quality figures at all.** Navidrome and any other OpenSubsonic server report `samplingRate`, `bitDepth` and `channelCount` on every song; koan's client did not parse them and the sync hardcoded all three to null. Every remote-only track in a synced library therefore had no sample rate and no bit depth — 5,058 of 5,058 in the library this was found on — so the format badge had nothing to show for them. For a player whose point is bit-perfect output, that is the wrong field to be missing. A full sync fills them in; a plain Subsonic server that does not report them still leaves them absent, because a missing sample rate is not 0 Hz.

- **A fresh HTTP client, and a fresh TLS handshake, for every remote request.** `subsonic_client()` built a new `SubsonicClient` on each call, and each of those builds two blocking `reqwest` clients — two runtimes on two threads with two cold connection pools. Browsing a synced library paid that per album cover. The download queue had already worked this out and kept a client of its own for the app's lifetime; the client is now shared process-wide and keyed on the credentials, so every caller gets connection reuse and logging in as someone else still replaces it.

- **Reading the config forked a `git` process.** `Config::load()` re-read both TOML files, re-ran the figment merge, and re-scanned for credentials in version control — and that last check shells out to `git ls-files` whenever a password is present in the file, then panics if it is tracked. koan reaches config from paths that run per frame: the macOS settings pane reads `library_folders()` from a SwiftUI list body, so it did all of that per rendered frame. The check is what its own message says it is, a gate on starting, and runs once per process now. The merged config is cached and re-read when either file's mtime moves, so a config edited by hand is still picked up.

## v0.26.0 (2026-08-23)

### Added

- **Play history.** koan never recorded its own plays: `play_history` existed for the radio feature, and its only production writer was the inbound Subsonic scrobble route — i.e. plays arrived when *other* clients scrobbled to koan, and playing a track in koan itself wrote nothing.

  A track is now written to history the moment it starts, not once it has been listened to for long enough. History answers "what did I put on, and in what order"; putting something on and skipping two seconds in is still a thing you did, and a log with a threshold on it is a log with holes in it. How long it was actually heard for is filled in afterwards, from position deltas — so a pause adds nothing and a seek does not credit the stretch it skipped.

  The macOS app gains a **History** section (⌘5) grouped by day. Read-only: rows link through to the album and the artist and carry the usual play/queue menu, but the only thing you can change is the log itself — select and ⌫ to forget entries, the same as queue items.

  Plays of tracks that came from a remote server are scrobbled to it. `SubsonicClient::scrobble` had been written and never called.

- **Albums and artists can be favourited, not only tracks.** Subsonic stars all three and koan only ever read songs back, so an album starred in Navidrome was invisible here and there was no way to star one from koan at all. There is a heart on an album tile, on an artist row, and in the header of both detail pages, and the context menu favourites the thing you opened it on rather than looping over its tracks — which would have turned off every track that was already a favourite. Favourites are keyed by name rather than row id, so like track favourites they survive a rebuilt index.

- **A heart on every queue row**, on hover, filled when the track is a favourite.

- **koan can be set up from the app.** Settings is four panes — Library, Server, Playback, Radio — covering everything needed to go from a fresh install to playing music without opening a terminal: library folders through a folder picker, signing in to a Subsonic or Navidrome server, transcode quality, cache limit and location, download workers, ReplayGain mode and pre-amp, radio lookahead and discovery. The configuration is still `config.toml`, shared with the CLI and the TUI, so this is a view onto that file rather than a second source of truth: fields commit when you finish editing, not on every keystroke, and the window re-reads on focus so a change made elsewhere is not silently overwritten.

- **Remote credentials go to the platform credential store.** `set_remote_credentials` checks them against the server before writing anything, stores the password in the keychain, and empties the config copy — so signing in once migrates a setup that had it in plaintext. Shared by the CLI and the app, which cannot now disagree about where credentials live.

- **The library keeps itself current.** An incremental scan a few seconds after startup, a rescan when the library folders change, and an incremental sync with the server on a timer. All three are cheap: 48,000 files walk in under a second and everything unchanged is skipped on its mtime and size, so the cost tracks what actually changed. Automatic sync is configurable, and a full sync stays a deliberate choice.

- **One place that says what koan is busy with.** Scans, syncs, library rebuilds and large queue edits each get a row at the foot of the sidebar with a label, how far through as counts rather than only a percentage, a bar, and what it is working on that second. Scans report a real fraction: the file count gives a denominator and a new progress callback carries the numerator across the FFI, throttled so a fifty-thousand-file scan does not cross it fifty thousand times. Only one library task runs at a time — they all queue behind SQLite's single writer — and the ones that cannot start are disabled rather than silently ignored. Each can be cancelled, which stops between transactions and keeps what it had already committed. ([#219](https://github.com/radiosilence/koan/issues/219))

- **Removing a folder or signing out can forget what it brought.** Both ask. A track held both locally and on the server keeps its row and loses only the half you removed, so "remove every folder and sign out" ends at an empty library, which is what it looks like it should do.

- **`just macos-signing-cert`** creates the self-signed certificate development builds sign with. Ad-hoc signing derives the app's identity from the binary's own hash, so every rebuild is a different application to macOS and keychain grants and TCC permissions are both forgotten. It does nothing for Gatekeeper — that needs Developer ID and notarisation.

### Changed

- **The app is `kōan.app`** and calls itself kōan in the menu bar and Finder. The executable inside stays ASCII, which is what `ps` and crash reports show.

- **Playback position is saved every second**, so a crash costs a second rather than the session. The queue is only rewritten when the queue changes: it is stored as a JSON blob, and re-serialising a library-sized one every second to remember a number is not a trade.

### Fixed

- **`play_history.track_id` was missing its `ON DELETE CASCADE`.** Under `foreign_keys = ON` a bare `REFERENCES` makes a track with history undeletable unless the caller clears the history first. One caller did; the constraint should not depend on the next one remembering. The table is rebuilt on first open, dropping entries whose track had already gone.

- **A sync never recorded the album's or the artist's id on the server.** The track's was kept and theirs was dropped, so all 5,800 albums in a synced library had a null `remote_id`. The server keys stars, shares and cover art off those ids, which left koan able to name an album but not refer to it. A full sync backfills them.

- **Favouriting a track from anywhere but the Favourites list appeared to do nothing.** The favourite was written and pushed to the server, but every view held its own copy of the track with the state baked in from when it was fetched, and only the Favourites list was reloaded — so the heart on the album page stayed empty on a track that was now a favourite. Removing one appeared to work because that is the one list that was refreshed. Favourite state now lives in one place and the row reflects it on the click rather than a round trip later.

- **The app never reconciled favourites with the server.** That only happened in `koan remote sync` on the command line, so a star made on another machine never arrived and one made in the app only left if you happened to run the CLI. It is part of a sync now, both directions, for all three kinds — union rather than mirror, since neither side records an unstar and treating one as authoritative would quietly delete favourites made on the other.

- **Nothing the engine logged was written down when it was hosted by the app.** koan-ffi never installed a logger, so every warning in koan-core — a favourite that could not reach the server, a file that would not decode — was discarded. It writes to `~/.config/koan/koan.log`, the same file the CLI uses.

- **The sidebar went on pointing at where you started.** Reaching an album from Favourites, from search, or from "Go to Album" in the queue left the selection on the section you came from. It follows the detail stack now: an album highlights Albums, an artist highlights Artists, whichever door you came through.

- **Artwork opens at the size of the window.** It was capped at 760pt, which made a 4000px scan no bigger than a thumbnail on a large display; lifting the cap then left it tall and narrow, square cover in the middle of a column of dead space. The sheet is sized from the window it hangs off rather than from its own proposal, which AppKit answers with a tall box.

- **"Show in Library" in the queue is "Go to Album"**, which is what it does and what the same action is called everywhere else.

- **`cargo test` asked for the login keychain password on every run.** A keychain item's ACL is keyed on the reading binary's code signature, and a cargo test binary is unsigned and rebuilt under a fresh hash each compile — so no ACL can match it, and "Always Allow" grants access to a binary that is about to stop existing. `KOAN_NO_KEYCHAIN=1` opts out and `just check` exports it.

- **The credential store is asked once per process rather than once per client.** `subsonic_client` builds credentials from scratch and the download queue, radio, sync and sharing each build one, so a single session asked several times over — and being asked five times for one password is indistinguishable from the app being broken.

- **One implementation of pushing a favourite to the server.** The TUI, the app and the server each had a byte-identical copy.

## v0.25.2 (2026-08-23)

### Fixed

- **The macOS app is universal again.** `swift build --arch arm64 --arch x86_64` puts its lipo'd product under `.build/apple/Products` and leaves `.build/release` pointing at one slice, so the bundle shipped the arm64 slice out of a build that had just compiled x86_64 as well — v0.25.1 does not run on Intel.
- **A broken bundle can no longer ship.** `just macos-verify <arches>` asserts the binary contains every architecture asked for and does not link `koan_ffi` dynamically, and CI runs it between the build and the DMG. Both bundles that went out broken today built, signed and packaged without complaint.
- **The Homebrew cask is no longer published with an empty checksum.** The shas came from bare `sha256sum <path>` calls whose failure went nowhere, so a missing artifact produced `sha256 ""` — which Homebrew refuses to install — and nothing in the run said so.

### Changed

- **Direct downloads say how to get past Gatekeeper.** The app is signed but not notarised, so macOS refuses the first open of a downloaded copy and offers only "Move to Trash". The `xattr -dr com.apple.quarantine` line now leads the release notes and the README's install section; the Homebrew cask already did this in a postflight.

## v0.25.1 (2026-08-23)

### Fixed

- **The v0.25.0 macOS app could not launch.** `-lkoan_ffi` over cargo's output directory finds the `.dylib` next to the archive and prefers it, so the shipped app referenced `/Users/runner/work/koan/koan/target/release/deps/libkoan_ffi.dylib` — a path that exists only on the CI runner — and was arm64-only, the host dylib having won over the universal archive. `just macos-ffi` now stages the right archive in a directory holding nothing else, which cannot produce either outcome. The bundled binary goes from 3.4 MB to 20 MB, which is the engine actually being in it.

## v0.25.0 (2026-08-23)

### Added

- **A native macOS app** (`apps/macos`) — SwiftUI on top of koan-core through new uniffi bindings (`koan-ffi`), not a client of the GraphQL server. The app links the engine in-process, so there is no daemon, no port and no auth surface between the UI and the audio it is driving. GraphQL remains the surface for clients that genuinely cannot link the core.

  Queue, albums, artists, favourites and snapshots, with album-grouped queue editing, drag and drop, a ⌘K picker, synced lyrics, global search, media keys and Now Playing, output device switching, cover art with a disk cache, and session restore that picks up mid-track — resuming only if playback was running when you quit.

- **`ReplacePlaylist { items, start }`** — replace the queue and choose where it starts, as one command. Clear-then-add-then-play is three commands down a bounded channel, each acted on as it arrives, so playing track nine of an album audibly started track one first. `replaceQueue` and the FFI both take the index.

- **Radio survives a restart**, and playback state records whether it was running, so reopening resumes rather than always landing paused.

- **Album and artist counts on artist rows**, and `added_at` on albums for a recently-added ordering.

### Changed

- **Sorting reads like a person would.** SQLite's default collation is a byte comparison, so the artist list ran `Zebra` before `aphex twin` and put `Âme` after the entire alphabet. A `LIBRARY` collation folds case and accents onto the base letter and compares digit runs as numbers, so `Track 2` precedes `Track 10`.

- **Radio no longer touches the network on the path that has to produce a track.** `pick_tracks` called ListenBrainz and MusicBrainz in line, and both rate-limit to one request a second per seed artist, so a queue with nothing left to play got its next track long after the music had stopped. It now uses local signals only — genre and era, same-artist, acoustic similarity and random — all database reads. The network signals remain for a background pass that fills the similar-artists cache.

- **Queue mutations are batched everywhere.** Removing a selection took the playlist lock and bumped the version per track, and building queue items re-read `config.toml` for every one. Sharing resolved remote ids one query per track.

### Fixed

- **Subsonic error bodies were cached as audio** — Subsonic reports failure with HTTP 200 and a JSON body, so a success status proves nothing on a binary endpoint. Navidrome's `data not found` for a stale id wrote a few hundred bytes of JSON into the cache, marked the track Ready, and then failed to decode forever — silently, and without ever retrying. Downloads now reject error documents before writing anything, and cache entries too small or too document-shaped to be audio are discarded and re-fetched. Distinct from the truncated-download fix below, which only catches transfers that end early.
- **Favourites were invisible on remote-only libraries** — `track_id_by_path` matches `path`, `cached_path` *or* `remote_url`, but the favourite check compared only the two local paths. A never-cached remote track could therefore be favourited and never show as one, and toggling one failed outright for want of a local path to key on.
- **Reaching the end of a queue crashed the player** — the render callback zeroes the tail of the output buffer when the ring runs dry, and passed a byte count to `ptr::write_bytes`, which counts in elements of the pointee. It wrote four times the intended range, off the end of the buffer CoreAudio had handed it. Nothing faulted: it corrupted the neighbouring block, and the damage only surfaced when CoreAudio freed it — a trap inside caulk's allocator during `AudioUnitUninitialize`, which read as a double free in the teardown rather than an overrun a track earlier. Underrun happens at the end of every track, so the end of a queue is where it landed.
- **ID3v1 tags won over ID3v2, truncating titles to 30 bytes** — a regression from the Symphonia 0.6 migration. 0.5 exposed probe-level and container-level metadata separately; 0.6 replaced both with one revision log, and the port walked it taking the first value it found for each field. For an MP3 carrying both tags the first revision is ID3v1, whose fields are a fixed 30 bytes, so `Golden Skans (David E Sugar Remix)` was indexed as `Golden Skans (David E Sugar R`. Only reachable for files lofty cannot parse — but those are exactly the files with damaged tags, and a truncated title matches nothing on a remote server, so the local copy of a record could no longer merge with the server's and appeared twice. Later revisions now win.
- **Scanning decoded every embedded cover image and threw it away** — `lofty::read_from_path` parses pictures by default, so a scan pulled a few hundred KB of JPEG out of every ID3v2 tag and FLAC block purely to drop it; a sampled `--force` run spent most of its time in `attached_picture_frame::parse`. The scan now reads tags and audio properties only. Artwork is unaffected: `extract_cover_art` has always done its own read, for the one track that needs it.
- **The Symphonia fallback read only three tags** — when lofty cannot parse a file at all, the fallback took title, artist and album and dropped the track number, disc, date and genre Symphonia hands over in the same pass. A row with no track number cannot content-match the same track synced from a remote server, so a file landing on this path stayed a duplicate even once its tags read correctly. It now takes everything offered.
- **"Recently added" was topped by whenever the last scan ran** — locally-scanned albums had no date of their own, so they were stamped with `datetime('now')` at insert. Every local album therefore sat permanently above everything the server considered new, in a different date format that wouldn't sort against it either. A local album now dates from the earliest mtime among its files, written as the same ISO 8601 UTC string a Subsonic server uses for `created`, and rescanning keeps the earliest rather than freezing whichever file the first scan reached. Existing scan-time stamps are cleared on upgrade so the next scan refills them; server-supplied dates are untouched.

- **The scan overlaps tag reads with database writes.** Chunking reads and writes with a barrier between them left the disk idle for every write and the CPU idle for every read; reads now stream down a bounded channel while the main thread commits.

- **lofty's 16 MB allocation cap made whole formats unreadable** — one record with 4000x4000 cover art in its Vorbis comments exceeded it, so lofty refused every file on it and they fell to the Symphonia fallback: worse metadata, three parses instead of one. Raised to 256 MB.

- **Share links report why they failed.** Every surface collapsed all failures into "local-only tracks can't be shared", including a server that returned no URL for a share it had created. Resolution now lives in koan-core, with distinct errors, one query for the remote ids, and a partial share that says how much it left out.

## v0.24.0 (2026-08-22)

### Breaking

- **The Subsonic REST API now has its own credentials and is off by default.** It previously authenticated with `remote.username` plus the `[remote]` password — the user's actual Navidrome account password — and `/rest/*` was mounted outside the JWT middleware, so `auth_enabled = true` did nothing for the surface that streams every file in the library. The protocol sends `md5(password + salt)` with a client-chosen salt over whatever transport the client picked, so one captured `/rest/ping` on the LAN yielded a digest to crack offline — and cracking it owned the remote music account too.

  Migration:

  ```bash
  koan subsonic setup     # generates a 256-bit secret, stores it in the keychain, prints it once
  ```

  Then re-point your Subsonic clients at the new username (`koan` by default) and secret. `koan subsonic status` shows the current state, `koan subsonic disable` turns it back off. Nothing is served at `/rest/*` until you run setup.

- **Plaintext Subsonic auth (`p=`, including `p=enc:...`) is refused.** Token auth (`u` + `t` + `s`) only. Every current client uses it.

- **`updateConfig` no longer accepts `libraryFolders` or `remoteUrl`.** Chained with `triggerScan` and `organizeExecute`, `libraryFolders` was a remote move of arbitrary files into the music tree; `remoteUrl` repointed sync at an attacker's server. Both stay CLI-only.

- **The `organize*` mutations are gated behind `[graphql] allow_organize = true`** (default `false`). They physically rename and move files.

- **Existing refresh tokens are invalidated.** They are now stored as `sha256(token)`; rows written under the old scheme no longer match. Clients re-login once.

- **Default CORS no longer allows any origin.** With `cors_origins` empty the server emits no `Access-Control-Allow-Origin` at all. List the origins your web client is served from.

- **The MCP `graphql` tool executes at `user` role**, not admin. It could previously invoke `organizeExecute`, `organizeUndo`, `updateConfig`, `triggerScan` and `createShare` — none of which its tool description advertised. `KOAN_MCP_ADMIN=1` restores admin; `setDevice`/`clearDevice`/`triggerScan` need it.

### Added

- **Native macOS app** — a SwiftUI front-end in `apps/macos`, built on a new `koan-ffi` crate that exposes `koan-core` to Swift via uniffi. In-process: no daemon, no port, no auth surface, and CoreAudio output stays in Rust so playback is bit-perfect. Queue-centric like the TUI, with album-grouped queue, a multi-select picker (add / add-and-play / replace queue), library and artist browsing, favourites, snapshots and synced lyrics. Visualizers are out of scope. Ships as `brew install --cask radiosilence/koan/koan-app`.
- **`SubsonicClient::get_cover_art`** — fetches artwork from the remote server. Libraries synced from Navidrome have no local files to read embedded tags out of, so every album was previously blank in any client relying on tag extraction.
- **`queries::favourite_track_ids_batch`** — favourited track IDs in one query.

### Security

- **h2 unbounded empty DATA frames (RUSTSEC-2026-0258)** — `axum::serve` runs hyper-util's auto builder, which speaks h2c on a plaintext listener, so anyone able to reach the API port could grow server memory without limit. h2 is now 0.4.18 (patched in 0.4.16).
- **rustls-webpki CRL panic and name-constraint bypasses (RUSTSEC-2026-0104, -0098, -0099, -0049)** — a hostile certificate chain from a Subsonic/Navidrome server could panic the sync client, and URI/wildcard name constraints were mis-evaluated. rustls-webpki is now 0.103.15.

- **Cross-site WebSocket hijacking** — `/graphql/ws` upgraded with no `Origin` check. WebSocket handshakes are exempt from CORS, so any page the owner visited could open a socket, have the browser attach the session cookie, and run queries *and* mutations while reading every response. Requests carrying a foreign `Origin` are now refused before the upgrade; an absent `Origin` (non-browser client) still passes.
- **CSRF via a CORS-safelisted content type** — `POST /graphql` with `Content-Type: text/plain` needs no preflight, so the browser sent it with cookies attached, and async-graphql parsed the body as JSON regardless. The response was unreadable but the mutation landed. `POST /graphql` now requires `application/json` or `application/graphql`.
- **DNS rebinding** — no `Host` was ever checked, so a page whose DNS flipped to `127.0.0.1` after loading reached the API as same-origin, at which point CORS, Private Network Access and `SameSite` are all irrelevant. Requests are now refused unless the `Host` is `localhost`, a bare IP literal, or listed in `[graphql] allowed_hosts`.
- **Session cookies were `SameSite=None; Secure`** — `None` is what made the two attacks above reachable, and `Secure` means browsers refuse to *store* the cookie over plain `http://` on a LAN address, so cookie auth was silently dead in the deployment the docs recommend. Now `SameSite=Lax`, with `Secure` only when `[graphql] cookie_secure = true`. The refresh token gets its own `HttpOnly` cookie scoped to `Path=/auth/refresh`.
- **`/auth/login` was unauthenticated, unthrottled and outside the concurrency limit** — Argon2 at 19MiB per verification meant a few hundred concurrent logins exhausted memory and pegged the CPU, and the handler ran the hash on the async workers, stalling every other request. Now: a concurrency limit of 2 on the auth router, a per-IP rate limit (10/minute), and `spawn_blocking` around verification.
- **An empty or truncated keypair silently downgraded to no-auth** — `auth_enabled && !private_pem.is_empty()` meant a damaged key file made every request an anonymous admin. It is now a startup failure. In TUI mode that failure is logged rather than panicking a background thread, where it left the API silently absent.
- **`getCoverArt?size=` was unbounded and upscaled** — `?size=65535` on a 1000×1000 cover asked for ~17GB, and `Vec`'s allocation failure aborts the process, which no panic handler can intercept. Clamped to 16–2048 and never larger than the source.
- **`?token=` applied to every route** — a valid JWT could sit in any URL, and from there in shell history, proxy logs and `Referer`. It exists for WebSocket URLs, which cannot carry a header, and is now confined to `/graphql/ws`.
- **The playground introspection key was a UUIDv7** — 48 of its bits were the server start time in milliseconds. It is now 256 bits from the system CSPRNG, and compared in constant time.
- **Refresh tokens were stored as plaintext UUIDs.** Now `sha256(token)`, and the tokens themselves are CSPRNG values rather than UUIDs.
- **GraphQL errors returned raw SQLite messages and absolute filesystem paths.** The detail is logged; clients get "internal error".
- **No query depth or complexity limit** — a single nested query could fan out across the whole library. Now `limit_depth(12)`, `limit_complexity(2000)`.
- **Subsonic username was compared with `!=`** while both password paths correctly used `subtle`. Now constant-time.
- **`parse_range` underflowed on a 0-byte track file** — panic in debug, `u64::MAX` in release. Guarded.
- **Dependency refresh across the workspace.** Notable majors: rusqlite 0.40, keyring 4, jsonwebtoken 11, rtrb 0.4, cpal 0.18, rmcp 3.1, bliss-audio 0.13, lofty 0.25, base64 0.23, tower-http 0.7, pem 4, getrandom 0.4, toml 1.1, jwalk 0.9, core-foundation 0.10, clap 4.6.
- **jsonwebtoken now uses the aws-lc-rs backend**, shared with rustls rather than pulling a second crypto stack. Token verification is stricter than under 9.x: the signature check can no longer be disabled, and the `alg` header is matched against the pinned `EdDSA` unconditionally.
- **keyring on Linux talks to secret-service over zbus** instead of dbus-secret-service. macOS Keychain access is unchanged — same generic-password service/account attributes and the same user keychain — so existing credentials still resolve.
- **bliss-audio no longer needs the aubio C library**; upstream replaced it with a Rust implementation, so `bliss-audio-aubio-rs` and its bindgen build are gone. The feature vector is unchanged (`FeaturesVersion::Version2`), so stored embeddings stay valid, though decoded values may shift marginally after bliss's Symphonia/Rubato update.

### Added

- **`koan scan --force-remove`** — deletes stale tracks even when the proportion missing trips the mount-failure brake, for the case where the files really were deleted. It lifts that one check and nothing else: a folder yielding no audio files is still left alone, and a path that cannot be stat'd is still not "gone". The run announces itself up front and lists what it removed.

- **Render tests for the TUI** (`crates/koan-tui/tests/render.rs`) — the widget layer had no test coverage, and layout and unicode regressions compile cleanly while rendering wrong. Pins the main layout split at every terminal height, asserts the seek bar's click hit-test agrees with the columns actually painted, and sweeps every widget across terminal sizes from 1×1 upward with titles containing CJK, emoji, ZWJ sequences, combining marks and RTL text.

Seven ways `koan organize` could destroy music files, every one of which was reported as a successful move.

- **Destinations are never overwritten.** `move_file` was a bare `fs::rename`, which silently replaces whatever is at the destination. Two rips of the same track, a case-only difference on macOS (`Rain` vs `RAIN`), or a download landing in the library mid-run all destroyed a file and reported success. Planning now refuses a destination claimed twice in one run or already occupied on disk, and `move_file` reserves the name atomically with `create_new` so nothing can slip into the gap. A case-only rename goes via a temporary name, since reserving the destination would otherwise open the source itself.
- **An empty path component no longer collapses an album onto one filename.** An unresolvable function was dropped from the output without marking the expression unresolved, and the path sanitiser skipped empty components — so `%artist%/%album%/$nun(%tracknumber%,2). %title%`, one typo, renamed every track on the album to `Artist/Album.flac`, each overwriting the last. Unknown function names are now a parse error, an unresolvable function marks its expression unresolved, and an empty, `.` or `..` component is refused instead of skipped.
- **Preview and execute read the same metadata.** Preview resolved fields from the database and execute from file tags, and the two didn't populate the same fields: `%label%` existed only in the tag path, so the shipped `$if2(%label%,%album artist%)` pattern previewed one tree and wrote another. Both now go through one resolver, with `label` and `date` coming off the album row.
- **The TUI's organize updates the database and can be undone.** It used a path-only code path that wrote no `organize_log` rows and left `tracks.path` pointing at the old locations — the queue looked right until the next launch, and a 5,000-file organize could not be undone. It now runs the same database-backed path as the GraphQL API; files the library has no row for are logged with a null `track_id` so undo still covers them.
- **Empty-directory cleanup can't delete a library root.** It walked up from the source directory with no floor, so organizing out of a configured library folder removed the folder itself. It now stops at the directories it was given.
- **Undo refuses rather than clobbers.** It never checked whether the original path had been re-occupied, picked its batch by a one-second timestamp that two batches could share, and aborted the whole run on the first failure while deleting the rows it had already processed. Batches are now chosen by primary key, each entry is restored only when its original path is free and the moved file still matches the size and modification time recorded for it, and per-entry failures are reported with their log rows left in place.
- **Path-keyed state follows the file.** Only `tracks.path` and `scan_cache.path` were rewritten, orphaning favourites (keyed by path), queue snapshots, saved playback state and cached paths. All of them are now rewritten in the same transaction as the move — and the database work happens first, so a `UNIQUE(path)` violation aborts before the file is touched instead of after.

Also hardened, same blast radius:

- **Cross-filesystem moves are durable.** `fs::copy` followed by `remove_file` meant power loss between the two left a zero-length destination and no source. The copy now goes to a temporary file, is flushed with `sync_all`, has its length verified and its modification time restored, and only then replaces the destination and unlinks the original. A run that won't fit on the target filesystem is refused before it starts.
- **The format parser can't be made to abort the process.** Nesting is capped at 64 levels (a few thousand nested `[` was a stack overflow, uncatchable, terminal left in raw mode), length-driven functions (`$repeat`, `$pad`, `$num`, `$tab`) cap their allocations, and `$add`/`$sub`/`$mul` use checked arithmetic.
- **A `)` inside a quoted argument no longer truncates a call.** The end of a function call is now found by the argument parser, which understands quoting, instead of a naive paren count that ended mid-expression and re-parsed the tail as a literal — silently appending garbage to the path.
- **Path components are length-capped** at the same 240 bytes as the rest of the codebase, so a long title is shortened rather than previewing cleanly and failing with `ENAMETOOLONG`.
- **Release pipeline could not recover from a partial crates.io publish.** The idempotency guard used
  `curl -sf` against the crates.io API, which returns 403 to curl's default User-Agent under its
  data-access policy — so the guard never fired and a retry after a partial publish always failed on
  "crate version already uploaded". The only escape was another version bump, which is what v0.23.2 and
  v0.23.3 were. Replaced the hand-rolled loop with `cargo publish --workspace`, which orders by
  dependency and waits for the index itself, and dropped `--no-verify` so the packaged crate is
  actually built.
- **The git tag was cut before crates.io.** Once tagged, `check-version` saw the release as done and
  set `should_release=false` forever, so crates.io could never be retried. crates.io now publishes
  first and the tag marks a release that actually shipped.
- Documented `auth_enabled` default was wrong in four places (it defaults to **true**), and the v0.22.0
  changelog entry contradicted itself. The guides that show how to disable auth now warn that
  doing so leaves the API open to anything that can reach the port.
- `ARCHITECTURE.md` and `CLAUDE.md` claimed the koan-tui/koan-server boundary was compiler-enforced. It
  is not — koan-tui declares an (unused) dependency on koan-server.

- MSRV declared (`rust-version = "1.89"`, set by async-graphql) and enforced by a CI job.
- `cargo audit` job and Dependabot for both cargo and GitHub Actions.
- Release gate that fails if the internal path-dep versions drift from the workspace version — the
  drift that `--no-verify` was hiding.
- Job timeouts, and `.gitignore` entries for local secrets and state.

- `koan subsonic setup|status|disable`.
- `[graphql] allowed_hosts`, `cookie_secure`, `allow_organize`.
- `[subsonic] enabled`, `username`, `password`.
- Tests for `auth/middleware.rs`, which previously had none: the cookie/Bearer/query-param precedence chain, the `auth_enabled = false` admin bypass, the introspection-key bypass and its constant-time mismatch, the `unwrap_or(Role::Readonly)` fallback on an unparseable role claim, tokens signed by another key. Plus the WebSocket `Origin` check (foreign rejected, absent allowed, same-origin and configured allowed), the `text/plain` POST rejection, the `Host` allowlist, cover-art size clamping, the empty-file range guard, cookie flags, and the login rate limiter.

### Changed

- `koan-core`: `SubsonicAuth` splits the credentials and URL signing out of `SubsonicClient`, which constructs blocking `reqwest` clients and so cannot be built inside a tokio runtime. `SubsonicClient::stream_to_file` fetches through `/rest/stream` for servers that implement only that. `GraphQLClient::stream_url` is gone — it was unused and omitted the auth parameters `/rest/*` requires.

- **Symphonia 0.5 → 0.6.1** — 0.6 rebuilt the format/codec registry, audio primitives, and metadata types around multi-track (audio/video/subtitle) media. Track timing moved off `CodecParameters` onto `Track`, which is where the audible wins come from: 24-bit/96 kHz ALAC now reports 96 kHz instead of 48 kHz (it previously played at half speed, and switched the output device to the wrong rate — fatal for a bit-perfect player), and ALAC-in-CAF decodes at all instead of erroring out. Playback frame counts are byte-identical across every other format.

- **GraphQL collections default to 50 rows and cap at 500.** A query with no `first` used to return
  the entire collection, so `{ tracks { edges { node { title } } } }` materialised a whole library as
  rows, as GraphQL values and as serialised JSON at once. Clients that relied on the unbounded form
  must paginate with `first`/`after`.
- **`triggerScan` and `triggerRemoteSync` return a `Job`, not a result.** Both run for minutes; they
  now start a detached worker and hand back `{ id, kind, state, message }`, polled with the new
  `job(id:)` and `jobs` queries. One job of each kind runs at a time — a second call returns the
  running one. The old `ScanResult` type is gone; added/updated/unchanged counts arrive in the
  finished job's `message`.
- **`sortBy` and `sortDir` now do something.** They were declared, published in the SDL and to MCP,
  and silently dropped: a client asking for `sortBy: DATE` got DB order and no error.
- **GraphQL queries time out at 30s, shed load past 64 in flight (503) and survive a panicking
  resolver.** The concurrency limit alone queued the surplus, so an overloaded server answered
  everyone slowly instead of telling the excess to come back. Subscriptions are exempt from the
  timeout.

- **Dependency refresh across the workspace.** Notable majors: rusqlite 0.40, keyring 4, jsonwebtoken 11, rtrb 0.4, cpal 0.18, rmcp 3.1, bliss-audio 0.13, lofty 0.25, base64 0.23, tower-http 0.7, pem 4, getrandom 0.4, toml 1.1, jwalk 0.9, core-foundation 0.10, clap 4.6.
- **jsonwebtoken now uses the aws-lc-rs backend**, shared with rustls rather than pulling a second crypto stack. Token verification is stricter than under 9.x: the signature check can no longer be disabled, and the `alg` header is matched against the pinned `EdDSA` unconditionally.
- **keyring on Linux talks to secret-service over zbus** instead of dbus-secret-service. macOS Keychain access is unchanged — same generic-password service/account attributes and the same user keychain — so existing credentials still resolve.
- **bliss-audio no longer needs the aubio C library**; upstream replaced it with a Rust implementation, so `bliss-audio-aubio-rs` and its bindgen build are gone. The feature vector is unchanged (`FeaturesVersion::Version2`), so stored embeddings stay valid, though decoded values may shift marginally after bliss's Symphonia/Rubato update.

- **The library scan commits in 1000-file chunks** instead of one transaction spanning the whole run. Ctrl-C at 90% used to discard everything and restart from zero; committed chunks now land in `scan_cache` so the next run resumes. Peak memory drops to one chunk rather than the whole library's metadata, and other writers — favourites, queue save on quit, play counts, GraphQL mutations — no longer sit behind a minutes-long write lock. `busy_timeout` raised from 5s to 30s to match.
- **Stale removal runs in its own transaction** and propagates failures instead of logging them and committing anyway. Its refusal message names the folder, how many tracks are missing, out of how many, and the percentage, so an ambiguous case is diagnosable from the error alone.
- **`scan_folder` and `full_scan` take a `ScanOptions`** instead of a bare `force` flag, and `remove_stale_tracks` returns the paths it removed rather than a count.

- **Download timeouts are per-stall, not per-transfer** — the download client bounds connect (10s) and time between bytes (30s) with no total deadline, and retries transient failures three times with backoff. JSON API calls keep a separate client with a 30s total deadline, which is correct for a small body read in one go.
- **One `SubsonicClient` per app, not per download** — the client was rebuilt inside `download_track`, re-reading config and redoing the TLS handshake for every track.
- **`SubsonicClient::stream_url` and the auth path are fallible** — the salt is generated from OS entropy and a request without it now fails instead of falling back to anything predictable. The salt is sent alongside `md5(password + salt)`, so a guessable one would make the token precomputable from a captured exchange.
- **All remote downloads share one implementation** (`koan-core/src/remote/download.rs`) — there were three copies of "stream bytes to disk with progress" and only one wrote to a temp file and verified the result.

- **ratatui 0.29 → 0.30, crossterm 0.28 → 0.29** — the two move together because `ratatui-crossterm` defaults to crossterm 0.29; bumping ratatui alone resolves two crossterm versions and breaks at the backend boundary. No source changes: koan's ratatui surface is `Buffer`, `Rect`, `Style`, `Line`/`Span`, `Widget` and `Length`/`Min`/`Percentage` constraints, and every 0.30 breaking change lands elsewhere.

  0.30 splits into `ratatui-core`/`ratatui-widgets`/`ratatui-crossterm` and replaces the cassowary layout solver with kasuari. Solver output is unchanged for koan's constraint sets, and `Buffer`'s out-of-bounds policy still panics rather than clamping, so nothing that used to render now renders differently or silently truncates. The one behavioural change is that halfwidth katakana dakuten/handakuten (`U+FF9E`/`U+FF9F`) now measure one cell instead of zero, matching how terminals actually draw them.

  The optional `termwiz`/`termina` backends appear in `Cargo.lock` but stay out of the build graph.

- The pre-push hook no longer runs `git add -A && git commit --amend` when `cargo fmt` changes files —
  it swept unrelated working-tree changes into the user's commit. It now fails and asks. It also runs
  `--all-targets`, matching CI, so warnings in test code stop passing the hook and failing CI.

### Fixed

- **No modern Subsonic client could finish a library sync.** The JSON serialiser emitted every attribute as a string, so `getSong?f=json` returned `"duration": "240"` where the OpenSubsonic `Child` schema says int. Symfonium, Substreamer and Feishin all default to `f=json` with generated deserialisers and aborted on the first song; `isDir` was never emitted at all. Attribute values now carry their wire type — `duration`/`track`/`bitRate`/`year`/`songCount`/`albumCount` are ints, `isDir`/`public`/the `getUser` roles are booleans — and only JSON changes. **The XML wire format is byte-for-byte what it was**, plus the newly added attributes.
- **No client showed cover art anywhere.** Neither songs nor albums carried a `coverArt` id. They now do (`mf-<id>` and `al-<id>`), and `getCoverArt` resolves the prefix: it previously read every id as a *track* id, so `getCoverArt?id=5` meaning "album 5" silently served track 5's art. A bare numeric id still means a track. Rendered covers are cached (256 entries, keyed on id and size) — painting a 200-album grid used to decode 200 media files.
- **`createPlaylist` with any song returned a bare HTTP 400.** `serde_urlencoded`, which axum's `Query` extractor uses, cannot deserialise the repeated `songId` parameter into a `Vec`, so the request was rejected with a plain-text body before the handler ran and only empty playlists could be created. The query is now parsed directly. The response also carries the created playlist, as it has since Subsonic 1.14.0.
- **Playlist contents were invisible to XML clients.** `getPlaylist` emitted members as `<song>` where the schema says `<entry>`.
- **Unimplemented endpoints returned an empty HTTP 404** instead of a Subsonic error, which several clients read as a broken server and fail their connection test on. Anything unhandled under `/rest/` now answers with error code 70. `getIndexes`, `getMusicDirectory`, `getAlbumList`, `getUser` and `getScanStatus` are implemented rather than refused — the first two are how DSub and every file-browse client enumerate a library.
- **`koan play --server` never played audio.** The bridge built `/rest/stream?id=<queue-item UUID>` with no auth parameters, against a handler expecting a library row id, so every request was rejected before it ran. `NowPlayingTrack` and `QueueEntry` now expose `trackId`, and the bridge signs its requests with the `[subsonic]` credentials from the machine it runs on — set those to match the server's, or client mode has nothing to stream with.
- **`getGenres` returned blank genre names to XML clients** — the name was written as a `value` attribute, which is the JSON spelling; the XSD carries it as element text. It also loaded the entire tracks table to count genres, now one `GROUP BY`.
- **An unsatisfiable `Range` served the whole file with a 200** instead of a 416 with `Content-Range: bytes */<total>`. A `Range` header that does not parse is still ignored, per RFC 9110.
- **A malformed `id` came back as a bare HTTP 400** from axum's extractor rather than a Subsonic error 10.
- **Database errors read as an empty library.** `getAlbum`, `getGenres`, `getPlaylists` and the artist album counts swallowed failures and answered `status="ok"` with zero rows, which tells a client to drop its cached library.
- **The Subsonic router was built twice at startup**, each build re-reading two config files and, on macOS, prompting the keychain. Proxied upstream streams also built a fresh HTTP client and re-read the config on every request; both are now resolved once.

- **The Linux audio callback took a mutex on every buffer.** The rtrb consumer was wrapped in a
  `std::sync::Mutex` so it could move into cpal's `FnMut` callback — but `rtrb::Consumer` is already
  `Send` and the callback bound is `FnMut + Send`, so a by-value capture was always enough. The lock
  cost an atomic read-modify-write per callback on the real-time thread, and its `try_lock` failure
  path filled the buffer with silence: an audible dropout reachable only through a contention that
  could not occur.
- **A panicking background job poisoned the job registry** for every later lookup, and a poisoned
  decode cursor silently stopped the gapless lookahead — stalling the queue with no error rather than
  failing loudly. Both now use `parking_lot`, which the project's own primitive table specifies.
- **A GraphQL query could stall audio in every connected Subsonic client.** rusqlite is blocking and
  nothing in the server ran it off the async runtime, so resolvers occupied tokio workers directly.
  Four concurrent `fuzzySearch` calls on a 4-core box took every worker, the `ReaderStream` feeding
  each in-flight `/rest/stream` response stopped producing bytes, and clients dropped the connection
  mid-track. One `triggerScan` did it single-handedly for the length of the scan. Every SQLite call,
  HTTP fetch, tag read and file decode now runs on the blocking pool.
- **A fresh SQLite connection was opened per resolver field.** `Database::open` creates the parent
  directory, chmods the file, sets four pragmas, attempts a WAL checkpoint and runs a ~30-statement
  DDL batch plus three migrations — all of it, every field. On a 500-artist library the nested
  artists → albums → tracks query cost roughly 3,500 open cycles, about 120,000 statements. The
  schema now holds a small connection pool sized to the core count, `Database::open_existing` skips
  the setup for pooled connections, and the DDL runs once.
- **N+1 queries across the type graph.** `Track.isFavourite` opened a connection and scanned the
  whole `favourites` table per track; `Album.trackCount` and `totalDurationMs` each materialised
  every row of the album to count or sum them; `Artist.albumCount`/`trackCount` re-ran the query
  their sibling field had just run. Counts and sums are now `COUNT(*)`/`SUM(...)` in SQLite, and
  parent → child edges go through dataloaders, so `{ tracks(first: 500) { isFavourite } }` is one
  query rather than 500 connections and 500 table scans.
- **`tracks(...)` loaded the whole library and filtered it in Rust.** Every predicate ran as a
  `retain()` over every row before pagination, `search:` silently truncated at 10,000 rows, and
  `yearStart`/`yearEnd` ran `SELECT date FROM albums WHERE id = ?` once per track in the library.
  Filters, ordering and the page window are now SQL with bound parameters.
- **`first: -1` overflowed the page arithmetic** — a panic in debug, a request for the entire
  library in release.
- **A full player command channel parked a tokio worker.** `send_cmd` used crossbeam's blocking
  `send` on a bounded(16) channel while the player can sit in `start_playback` for about a second
  during a device rate change. It now waits 250ms and reports "player busy".
- **`cargo test` overwrote the user's real JWT signing key.** `auth`'s keypair tests called
  `generate_keypair()`, which writes to `~/.config/koan/auth/`, so running the test suite rotated the
  live Ed25519 key and invalidated every issued token. Keypair derivation is now split from the
  filesystem write and the tests use the pure form.
- **MP3s at unusual sample rates played at the wrong speed** — the audio engine was configured with the rate the *device* settled on, not the rate the PCM actually is. Output devices reject the MPEG-2/2.5 rates that only MP3 uses (8/11.025/12/16/22.05/24 kHz, and 32 kHz on many DACs), so a 22.05 kHz MP3 on a 44.1 kHz device played at exactly double speed. The engine is now always configured from the source format and the device switch is a best-effort bit-perfect optimisation; when it fails the platform resamples instead. FLAC never hit this because it is only ever ripped at rates every device supports. ([#181](https://github.com/radiosilence/koan/pull/181))
- **Mixed-format queues played the second track at the wrong speed** — every track in a gapless session shares one ring buffer and therefore one engine, but the decode thread would happily push a 48 kHz track in behind a 44.1 kHz one. A track whose rate or channel count differs now ends the decode session so the player can restart it on a correctly configured engine.
- **Tail of the last decoded track was cut off** — the decode thread signalled completion as soon as it had *written* the last sample, up to 4 seconds before the audio engine had played it. It now waits for the ring buffer to drain first.
- **An empty or unmounted library folder deleted the entire library** — `full_scan` only checked that the folder existed, so a NAS mount that failed, an unattached Docker volume, or a directory whose permissions changed left an empty-but-present path. Stale removal then found every indexed path missing and deleted the rows along with their play history, lyrics and embeddings. Three brakes now: a folder yielding zero audio files skips stale removal entirely, `try_exists` means an IO error is never read as "deleted", and a run that would clear more than 20% of a folder holding at least 100 tracks is refused outright.
- **Scanning one folder swept its siblings** — the stale-removal prefix had no trailing separator, so scanning `/Volumes/Music` also matched `/Volumes/Music Backup`. Unplugging the backup drive and rescanning the main one deleted the backup's rows.
- **Content dedup merged distinct tracks and lost a file** — the match ignored `disc`, so a 2-CD box set whose discs share a track title and number collapsed into one row pointing at whichever disc was scanned last; the other file became unreachable in library, search and queue, and stale removal never noticed because the file was still on disk. `disc` is now part of the predicate, and the match only fires across sources: two rows that both carry a local path, or that both carry a remote id, are two tracks. That keeps the local↔remote dedup the design wants. The cost is that a server which rotates its ids yields visible duplicates instead of re-attaching silently — duplicates you can see and fix, where a swallowed track you can do neither with.
- **Remote sync erased locally-scanned audio properties** — merging wrote every column straight from the incoming metadata, so syncing against a Navidrome serving the same files nulled `sample_rate`, `bit_depth`, `channels`, `size_bytes` and `mtime` across the library and rewrote the codec. A merge now fills gaps only and never overwrites a populated column with NULL.
- **Orphaned `scan_cache` rows aborted stale cleanup half-done** — cleanup deleted the cache row by the track's current path, leaving any row under a former path behind. The foreign key then failed the `DELETE FROM tracks` — after the FTS, lyrics, play-history and embedding rows had already gone — and every remaining stale track in that run was skipped. Cache rows are now cleared by `track_id` as well as path.
- **A single panicking file aborted the whole scan** — lofty and symphonia can panic on hostile tags; rayon re-raised it at `collect()`, so one bad file out of 500k produced zero indexed tracks and a backtrace that didn't name it. Tag reads are contained; the file is reported as an error and the scan continues. Same for acoustic analysis.
- **Files skipped by walkdir vanished silently** — permission-denied subtrees and symlink loops were discarded without a word. They are logged, counted in `ScanResult::unreadable`, and reported by `koan scan`.
- **`ScanResult::updated` was always zero** — every upsert counted as `added`, so `koan scan` printed "0 updated" every run and GraphQL returned the same through `tracksUpdated`. `upsert_track_status` reports whether a row was inserted, which also makes `ScanEvent::is_new` truthful.
- **A failed `scan_cache` write was swallowed** — the track was indexed but uncached, so every future scan re-read its tags with no diagnostic.

- **Failed album fetches no longer become permanent library holes** — a sync that lost albums to network errors still reported success and advanced `last_sync`, so the next incremental sync skipped straight past them. `last_sync` now only advances when every album fetch succeeded, and `SyncResult` carries the failure count so `koan remote sync` and `triggerRemoteSync` report an incomplete run.
- **Sync pagination can no longer skip albums** — the offset walk used `type=newest`, whose ordering shifts whenever the server reorders or adds an album mid-sync. It now walks `alphabeticalByName`, de-duplicates album ids for the run, and uses `created` only to decide which albums need a detail fetch.
- **Truncated downloads can no longer masquerade as cached tracks** — the TUI remote bridge wrote straight to its destination and only checked completeness when the server sent a Content-Length, so a dropped connection on a chunked stream (Navidrome's transcoded output) left a truncated file that played as a stub for the rest of the session. Every remote download now goes through one implementation that writes a `.part` file and renames only on a verified-complete transfer.
- **The remote-stream cache is bounded** — bridge downloads were keyed on a per-session queue id, so nothing was ever reused and every play left a full-size file behind forever. They are now keyed on track identity and the directory is pruned to a 2GB budget.
- **Priority downloads respect `download_workers`** — cursor movement spawned an unbounded thread per landing, so scrolling a large remote queue fired hundreds of concurrent requests at the server. Priority downloads now run on a two-permit lane, tracks already downloading are never started twice, and anything over the limit goes to the head of the worker queue.
- **Favouriting a track mid-download sticks** — the star was keyed on the in-progress `.part` path, which stops existing when the download completes, so it silently disappeared and was never pushed to the server.
- **Download workers survive panics** — a panicking download permanently shrank the worker pool for the process lifetime.
- **Lost server connections are visible** — the remote bridge swallowed poll errors and froze on the last known state while retrying at 10Hz. Connection loss and recovery are now logged.

- **Deleting the playing track jumped back to the top of the queue** — removing an item clears the cursor, and an unset cursor means "start from the beginning", so deleting track 25 of 40 resumed at track 1. The removed track's predecessor is now pinned before advancing, so playback continues at its successor. Reachable from the TUI, GraphQL and MCP.
- **Deleting an upcoming track replayed the whole queue** — the decode thread's gapless lookahead runs up to ~4 seconds ahead of what is audible, and its reference item vanishing was read as "start of queue". Track 6 would run gaplessly into track 1. A missing reference now ends the lookahead so the audible track's own successor is used.
- **Playback died for good when the next track had not finished downloading.** Advancing only accepted fully-downloaded tracks and left the cursor where it was, and the download completion signal is discarded unless the cursor is on that track — so on a slow link the queue stopped with every track showing Ready and nothing resumed it. The cursor now parks on the track being fetched and playback resumes when its bytes land.
- **A failed stream download hung playback and leaked three threads.** The decode thread waited on a condvar with no timeout and no way to learn the download had died; the pump feeding it spun at 100 wakeups/sec forever, holding the whole buffered track (100+ MB for hi-res FLAC), and the engine teardown then blocked on joining it. Reads now fail with a broken-pipe or timeout error rather than a silent EOF that would truncate the track, and the pump exits when the download fails, stalls for 30s, or nothing is left to read it. A chunked transfer with no Content-Length no longer hangs at end of track.
- **Batch-deleting a selection containing the playing track restarted the audio engine once per deleted track** — each restart re-probes the file, re-enumerates devices and can change the DAC's sample rate, so Ctrl+A Delete on a long queue froze the player and clicked the output. The resume point is now resolved once for the whole selection.
- **Undo of a multi-track delete restored tracks in the wrong order.** The selection arrives from a `HashSet` in arbitrary order, and re-inserting a track whose recorded predecessor had not been restored yet appended it to the end. Positions are now snapshotted in playlist order.
- **A four-way lock cycle could hang the app in remote-bridge mode** — `current_download_fraction` took the track-info lock before the playlist lock while `derive_visible_queue` took them the other way round. Neither holds both any more.
- **A failed playback start left a zombie "Playing" transport** with a frozen position, since the engine had already been torn down by the time the failure surfaced. A failure now leaves the player cleanly stopped, and pause/resume report what the engine actually did instead of assuming success.
- **The visualizer ran up to 4.35 seconds ahead of the audio.** Every mode — spectrum, VU, oscilloscope, lissajous, beat-reactive colour — was drawing samples the DAC had not reached yet. The decode thread pushed into the viz buffer at the moment it wrote into the ring buffer, and a local FLAC decodes 50-100x realtime, so the ring stays saturated and a sample written at T is heard at T + ring_depth/rate: 4.35s at 44.1kHz, 4.00s at 48kHz, 2.00s at 96kHz, 1.00s at 192kHz. Nothing looked broken because the bars still moved in time. The viz buffer is now a delay line the length of the ring buffer, read at the position the audio engine's played counter reports rather than at the write head. Feeding it from the render callback was not an option — that thread may never allocate or lock.
- **The transport went blank for ~50-100ms on every skip and every seek.** `stop_engine` signals the decode thread and hands the join to a cleanup thread, so the outgoing thread was still mid-packet when the new session's `timeline.reset()` ran. Its final `add_written` then landed on the fresh timeline — one packet, 4608 interleaved samples for stereo MP3 and up to 8192 for FLAC — so the new track's first boundary was stamped at that offset instead of 0, and with the fresh engine's `samples_played` starting at 0 the binary search found nothing and `current_playback()` returned `None`. The same window admitted a phantom `push_boundary` from the dying thread, which would have shown the wrong track's metadata for the rest of the session. Timeline writes now go through a handle carrying the generation its session started in, checked under the same lock `reset()` takes; a retired handle's writes are dropped. The decode thread also polls it as a second abort signal, which stops a dying thread pushing into the visualizer delay line the next session has just reset.
- **Seeking reported the requested position, not where playback resumed.** The timeline recorded the seek target before the seek ran, and symphonia's returned landing point was discarded. Coarse seeking made this worse rather than exposing it: it picks a byte offset by interpolating linearly over the whole file and then derives its reported timestamp from that same guess, so a 5-minute VBR MP3 seeked to 2:30 resumed at 2:33.7 while reporting 2:29.9 — a 3.8-second lie for the rest of the track, and near the end the bar pinned at 100% while audio still played. Seeks are now accurate rather than coarse (1.5-3ms on files up to 79MB) and the boundary is pushed after the seek, from the real landing point.
- **One unreadable file killed the rest of the queue and truncated the track still playing.** A failure to open or decode any track ended the decode thread, which the player read as end-of-queue. Because the decode head runs up to a full ring buffer ahead of the DAC, track 5 of an album failing cut track 4 off ~4 seconds early and tracks 6-20 never played at all; a bad *first* track ended the session without trying anything else. Bad sources are now skipped with the path logged, with a 32-consecutive-failure cap so a wholly unreadable queue still terminates.
- **ReplayGain clipped hard whenever the peak tag was absent.** Gain was only limited when a peak tag existed, and nothing downstream clamps — engine.rs, cpal_backend.rs and opus.rs go straight to the DAC. A quiet classical recording tagged `REPLAYGAIN_TRACK_GAIN=+9.5 dB` with no peak gave ~2.99x, so everything above 0.33 FS clipped: gross continuous distortion on exactly the material ReplayGain exists to rescue. A negative peak from a malformed tag inverted phase. Only a finite, positive peak is now trusted to bound the gain, and the output is clamped to ±1.0 unconditionally.
- **`"+3.21 db"` silently disabled ReplayGain** — only a literal `"dB"` suffix was stripped, so a lower-case tag failed to parse and the file played unadjusted. The suffix match is now case-insensitive.
- **Spectrum read 6 dB low.** The FFT magnitude scale was `2/N`, correct for a rectangular window, but the analyzer applies a Hann window with a coherent gain of 0.5. Everything read 6.02 dB down, so against the -80 dB floor a full-scale sine topped out at 0.925 and the bars never reached the top of the widget. The scale is now derived from the window's own sum.
- **Spectrum bass bars combed and collapsed at high sample rates.** A fixed 2048-point FFT spaces bins 93.75 Hz apart at 192 kHz, leaving whole runs of the bottom Bark bars with no bin at all. The gap filler ran left-to-right in place, so it fed synthesised values into the next bar's average while the bar to the right was still zero: a sawtooth ripple biased low, with bar 0 sitting permanently at half height. Runs of empty bars are now interpolated in one pass between their measured neighbours.

### Removed

- **The ReplayGain scanner.** `scan_track`, `scan_album`, `write_tags` and their helpers had no caller anywhere and no CLI or GraphQL surface — and `scan_album` was wrong regardless: it built one R128 analyser from the first track's spec and fed every subsequent track through it, so a mono interlude or a 48 kHz bonus track silently corrupted the album gain for every track. Reading and applying ReplayGain tags during playback is untouched. Drops the `ebur128` dependency.
- **`playback.software_volume`.** Declared, defaulted, documented and tested, but read by nothing since it was added — setting it did nothing at all.

- **`cargo test` overwrote the user's real JWT signing key.** `auth`'s keypair tests called
  `generate_keypair()`, which writes to `~/.config/koan/auth/`, so running the test suite rotated the
  live Ed25519 key and invalidated every issued token. Keypair derivation is now split from the
  filesystem write and the tests use the pure form.
- **MP3s at unusual sample rates played at the wrong speed** — the audio engine was configured with the rate the *device* settled on, not the rate the PCM actually is. Output devices reject the MPEG-2/2.5 rates that only MP3 uses (8/11.025/12/16/22.05/24 kHz, and 32 kHz on many DACs), so a 22.05 kHz MP3 on a 44.1 kHz device played at exactly double speed. The engine is now always configured from the source format and the device switch is a best-effort bit-perfect optimisation; when it fails the platform resamples instead. FLAC never hit this because it is only ever ripped at rates every device supports. ([#181](https://github.com/radiosilence/koan/pull/181))
- **Mixed-format queues played the second track at the wrong speed** — every track in a gapless session shares one ring buffer and therefore one engine, but the decode thread would happily push a 48 kHz track in behind a 44.1 kHz one. A track whose rate or channel count differs now ends the decode session so the player can restart it on a correctly configured engine.
- **Tail of the last decoded track was cut off** — the decode thread signalled completion as soon as it had *written* the last sample, up to 4 seconds before the audio engine had played it. It now waits for the ring buffer to drain first.
- **An empty or unmounted library folder deleted the entire library** — `full_scan` only checked that the folder existed, so a NAS mount that failed, an unattached Docker volume, or a directory whose permissions changed left an empty-but-present path. Stale removal then found every indexed path missing and deleted the rows along with their play history, lyrics and embeddings. Three brakes now: a folder yielding zero audio files skips stale removal entirely, `try_exists` means an IO error is never read as "deleted", and a run that would clear more than 20% of a folder holding at least 100 tracks is refused outright.
- **Scanning one folder swept its siblings** — the stale-removal prefix had no trailing separator, so scanning `/Volumes/Music` also matched `/Volumes/Music Backup`. Unplugging the backup drive and rescanning the main one deleted the backup's rows.
- **Content dedup merged distinct tracks and lost a file** — the match ignored `disc`, so a 2-CD box set whose discs share a track title and number collapsed into one row pointing at whichever disc was scanned last; the other file became unreachable in library, search and queue, and stale removal never noticed because the file was still on disk. `disc` is now part of the predicate, and the match only fires across sources: two rows that both carry a local path, or that both carry a remote id, are two tracks. That keeps the local↔remote dedup the design wants. The cost is that a server which rotates its ids yields visible duplicates instead of re-attaching silently — duplicates you can see and fix, where a swallowed track you can do neither with.
- **Remote sync erased locally-scanned audio properties** — merging wrote every column straight from the incoming metadata, so syncing against a Navidrome serving the same files nulled `sample_rate`, `bit_depth`, `channels`, `size_bytes` and `mtime` across the library and rewrote the codec. A merge now fills gaps only and never overwrites a populated column with NULL.
- **Orphaned `scan_cache` rows aborted stale cleanup half-done** — cleanup deleted the cache row by the track's current path, leaving any row under a former path behind. The foreign key then failed the `DELETE FROM tracks` — after the FTS, lyrics, play-history and embedding rows had already gone — and every remaining stale track in that run was skipped. Cache rows are now cleared by `track_id` as well as path.
- **A single panicking file aborted the whole scan** — lofty and symphonia can panic on hostile tags; rayon re-raised it at `collect()`, so one bad file out of 500k produced zero indexed tracks and a backtrace that didn't name it. Tag reads are contained; the file is reported as an error and the scan continues. Same for acoustic analysis.
- **Files skipped by walkdir vanished silently** — permission-denied subtrees and symlink loops were discarded without a word. They are logged, counted in `ScanResult::unreadable`, and reported by `koan scan`.
- **`ScanResult::updated` was always zero** — every upsert counted as `added`, so `koan scan` printed "0 updated" every run and GraphQL returned the same through `tracksUpdated`. `upsert_track_status` reports whether a row was inserted, which also makes `ScanEvent::is_new` truthful.
- **A failed `scan_cache` write was swallowed** — the track was indexed but uncached, so every future scan re-read its tags with no diagnostic.

- **Failed album fetches no longer become permanent library holes** — a sync that lost albums to network errors still reported success and advanced `last_sync`, so the next incremental sync skipped straight past them. `last_sync` now only advances when every album fetch succeeded, and `SyncResult` carries the failure count so `koan remote sync` and `triggerRemoteSync` report an incomplete run.
- **Sync pagination can no longer skip albums** — the offset walk used `type=newest`, whose ordering shifts whenever the server reorders or adds an album mid-sync. It now walks `alphabeticalByName`, de-duplicates album ids for the run, and uses `created` only to decide which albums need a detail fetch.
- **Truncated downloads can no longer masquerade as cached tracks** — the TUI remote bridge wrote straight to its destination and only checked completeness when the server sent a Content-Length, so a dropped connection on a chunked stream (Navidrome's transcoded output) left a truncated file that played as a stub for the rest of the session. Every remote download now goes through one implementation that writes a `.part` file and renames only on a verified-complete transfer.
- **The remote-stream cache is bounded** — bridge downloads were keyed on a per-session queue id, so nothing was ever reused and every play left a full-size file behind forever. They are now keyed on track identity and the directory is pruned to a 2GB budget.
- **Priority downloads respect `download_workers`** — cursor movement spawned an unbounded thread per landing, so scrolling a large remote queue fired hundreds of concurrent requests at the server. Priority downloads now run on a two-permit lane, tracks already downloading are never started twice, and anything over the limit goes to the head of the worker queue.
- **Favouriting a track mid-download sticks** — the star was keyed on the in-progress `.part` path, which stops existing when the download completes, so it silently disappeared and was never pushed to the server.
- **Download workers survive panics** — a panicking download permanently shrank the worker pool for the process lifetime.
- **Lost server connections are visible** — the remote bridge swallowed poll errors and froze on the last known state while retrying at 10Hz. Connection loss and recovery are now logged.

- **`remove_track_by_path` and `remove_tracks_by_source`** — unused outside their own tests, and both left orphaned foreign-key rows behind that would fail a later delete.

### Internal

- **Internal crate versions are declared once**, in `[workspace.dependencies]`, instead of being pinned per member — the per-member pins had drifted a patch behind and would have published a broken `koan-cli` at the next minor bump.
- **`koan-tui` no longer depends on `koan-server`.** It never imported it; the dependency pulled axum, async-graphql, rmcp, tokio and tower into every TUI build and contradicted the documented crate boundary, which is now compiler-enforced again.
- **`reqwest` is declared once in `[workspace.dependencies]`** with the feature union the workspace already resolved to, replacing four declarations with four different feature sets.
- Dropped confirmed-unused deps: `toml`, `chrono`, `rayon`, `owo-colors` from koan-tui; `walkdir` from koan-server; `core-foundation`, `crossbeam-channel` from koan-cli.
- Tests covering the `ALTER TABLE` schema migrations: SQLite's "duplicate column" wording is the only thing stopping `Database::open` from failing on an already-migrated database, so it is now asserted rather than assumed.
- **Transport geometry ignored the layout solver, writing outside the frame buffer** — `render()` derived the album-art rect, the transport text rect and the spectrum rect from the height it *asked* for, not the height the solver granted. On a short terminal the solver shrinks the transport, the text rect lands below the buffer, and `Buffer::set_stringn` (which clamps x but never y) indexes straight off the end. Trigger is `width >= art_size + 2 && height <= art_size / 2 - 3` — at the default `art_size = 24` that is width >= 26 and height <= 9, so 80x24 was safe. Dragging the transport divider persists `art_size` up to 80 into `config.toml`, and at 80 the trigger covers ordinary terminals: koan then panicked on the first frame of every launch, with no way to fix it from inside the app. All three rects now come from the solver, and `render()` bails out with a message below 20x5.
- **FPS overlay underflowed on narrow terminals** — the counter is 14 cells wide (19 with the beat tag) but only checked for 8, so `area.x + area.width - w` wrapped.
- **Empty queue panicked in a one-row pane** — `QueueView` wrote its "empty" line at `block.inner(area).y` without checking the top border had left a row.
- **Album dates with multibyte characters panicked** — the library browser and album picker took `&date[..4]` to get the year. `len()` is bytes, and date tags are free-form text, so a Japanese or Chinese pressing (or fullwidth digits) split a character mid-sequence. Now counts characters.
- **Queue clicks resolved to the wrong row after an edit-mode scroll** — `QueueView` computed its own scroll offset in edit mode and never wrote it back, while `App` scrolled against a different height and every hit-test read the stale offset. Once they diverged, clicking a track selected the one above it for the rest of the session. There is now one definition of the queue's visible height, and the widget renders exactly the offset it is given.
- **All but the last mouse event in a frame were dropped** — coalescing every event meant a trackpad flick scrolled one line, and a click whose press and release landed in the same frame did nothing. Only motion coalesces now.
- **Terminal left in raw mode on I/O errors** — every `?` out of the event loop skipped the restore block. A `Drop` guard handles it.
- **Track info modal keyed off a queue index** — removing a track shifted it onto a different track, or past the end, where the modal rendered nothing while still swallowing every keystroke. It now holds a `QueueItemId` and closes when that track leaves the queue.
- **Library filter bar overlapped its own click region** — the view reserves the bottom row for the filter input while focused, but hit-testing and scrolling used the full height, so clicking the input selected a node.
- **Transport resize handle stole the queue's top border row.**
- **Queue scrollbar's last drawn row was not clickable** — clicking it fell through and selected a track.
- **Matroska duration** — MKV states its duration at media level in millisecond ticks, which was being read as a frame count and rendered a 10-second file as 226 ms. Durations now come from the container's stated duration via its own timebase, falling back to the playable frame count.
- **MP3 duration overstated by ~30 ms** — the probe reported the untrimmed frame count while the decoder dropped encoder delay and padding, so the seek bar ran past the end of the audio. Both sides now report the trimmed length.
- **Gapless trimming in ReplayGain scans** — encoder delay and padding are now dropped before loudness analysis, so MP3/Vorbis scans no longer measure the silence the decoder discards. Scanned gain values shift very slightly; re-scan to refresh them.

### Known issues

- **WAV `LIST INFO` tags are not read** — Symphonia 0.6.1's WAV reader parses the chunk into a metadata log and then builds the reader from `external_data` instead, discarding it. Only reachable for WAVs lofty cannot parse, since lofty reads these tags on the happy path; such files fall back to a filename-derived title.

Closes the browser-facing attack surface on `koan serve`. The threat model that drove this: koan on a LAN, reachable from other machines on the network and from any web page the owner's browser happens to load.

## v0.23.3 (2026-04-19)

### Fixed

- **`similarArtists` crashed on pre-existing DBs** — schema added a `relationship` column to `similar_artists` but shipped no `ALTER TABLE` migration, so databases created before the column existed blew up with `no such column: sa.relationship` the moment the query ran. Added the migration alongside the existing cache-column migrations and a regression test that boots a pre-migration schema and verifies `create_tables` patches it. ([#180](https://github.com/radiosilence/koan/pull/180))

## v0.23.2 (2026-04-18)

Second attempt at getting the split crates on crates.io. v0.23.1 published `koan-core` but then failed because the publish order had `koan-tui` before `koan-server` — and `koan-tui` depends on `koan-server`. Flipped the order; no code delta.

### Fixed

- **Publish order** — `koan-server` now publishes before `koan-tui` so the crates.io index sees the dependency before downstream crates try to resolve it.

## v0.23.1 (2026-04-18)

Re-release of v0.23.0 to publish the split crates to crates.io. No code changes from v0.23.0.

### Fixed

- **Missing crates on crates.io** — the publish-crate CI job referenced the pre-split binary name (`koan-music`) and silently failed for every release since v0.21.0. `koan-tui`, `koan-server`, and `koan-cli` had never been published. The job now publishes all four crates in dependency order and is idempotent across partial retries. ([#177](https://github.com/radiosilence/koan/pull/177))

## v0.23.0 (2026-04-18)

Groundwork for the upcoming browser SPA: full GraphQL schema for web clients, real-time subscriptions, cookie auth, and configurable CORS. Subsonic streaming now rides the GraphQL port by default, so the remote TUI (`koan play --server`) finally works end-to-end.

### Added

- **GraphQL subscriptions** — real-time data over WebSocket at `/graphql/ws`. Three subscriptions: `nowPlaying` (playback state at configurable interval), `queueUpdated` (full queue snapshot on change), `vizFrame` (spectrum/VU/waveform at configurable FPS). ([#162](https://github.com/radiosilence/koan/issues/162), [#171](https://github.com/radiosilence/koan/pull/171))
- **Queue snapshot with status** — `queue` query returns `QueueSnapshot` with versioned entries, each with derived `status` (Queued/Playing/Played/Downloading/PriorityPending/Failed) and optional `downloadProgress`. Replaces flat `Vec<QueueEntry>`.
- **Visualizer query** — `vizFrame` query returns current spectrum, peaks, VU levels, beat energy, and optional waveform. Returns null when no analyzer is running.
- **Config query + mutation** — `config` query exposes current settings, `updateConfig` mutation writes individual fields to `config.toml` (admin-only).
- **Playlist version query** — `playlistVersion` returns monotonic counter for change detection.
- **Playback state persistence** — `savePlaybackState` and `clearPlaybackState` mutations for web clients.
- **Cookie auth for web clients** — auth routes set `HttpOnly; Secure; SameSite=None` cookies alongside JSON responses. Middleware checks cookie before Bearer header (priority: cookie > Bearer > query param). Logout clears the cookie. No breaking changes for CLI/mobile clients. ([#172](https://github.com/radiosilence/koan/pull/172))
- **Configurable CORS origins** — `[graphql] cors_origins = [...]` restricts allowed origins with `credentials: true` for cookie auth. Empty (default) keeps `Allow-Origin: *` for dev/backward compat.

### Fixed

- **Subsonic streaming works on the GraphQL port** — Subsonic REST is now merged onto the GraphQL router whenever remote credentials are configured, so `koan play --server <url>` can pull streams without a second listener. `--subsonic <port>` continues to expose an additional dedicated listener for clients that expect a separate port. ([#174](https://github.com/radiosilence/koan/pull/174))
- **Rust 1.95 clippy lints** — resolved `collapsible_match`, `manual_checked_ops`, and `absurd_extreme_comparisons` across `koan-core` and `koan-tui`. Pure refactor; no behaviour change. ([#175](https://github.com/radiosilence/koan/pull/175))

### Known issues

- **Remote TUI client doesn't send a JWT yet** — `koan play --server <url>` still fails against `auth_enabled = true` servers because the client has no login/token flow. Tracked in [#173](https://github.com/radiosilence/koan/issues/173).

### Internal

- **`ApiServerOpts` struct** — replaces positional args on internal `run_api_blocking`, carries optional `VizSnapshot` for subscription support.

## v0.22.0 (2026-04-12)

### Added

- **API authentication** — JWT-based auth with Ed25519 signing for the GraphQL and Subsonic APIs. ([#161](https://github.com/radiosilence/koan/issues/161))
  - Three roles: `admin` (full control), `user` (playback, queue, favourites), `readonly` (browse-only).
  - Argon2id password hashing, short-lived access tokens (15min default), single-use rotating refresh tokens (30d default).
  - Auth routes: `POST /auth/login`, `POST /auth/refresh`, `POST /auth/logout`.
  - Axum middleware validates JWT on protected routes. When `auth_enabled = false`, all requests pass through as admin — zero breaking change for existing installs.
  - CLI commands: `koan auth setup` (keypair + first admin), `koan auth create-user`, `koan auth delete-user`, `koan auth list-users`, `koan auth login`, `koan auth logout`.
  - Refresh tokens stored in platform keychain via `keyring`. In-process execution (MCP) bypasses auth.
  - Config: `[graphql]` section gains `auth_enabled`, `access_token_ttl`, `refresh_token_ttl`.
  - DB tables: `users`, `refresh_tokens` (auto-created on startup).
  - Role-based guards on all GraphQL mutations (admin: scan/organize/device, user: playback/queue/favourites, readonly: queries only).
  - `koan auth reset-password <user>` — reset password, revoke all tokens.
  - `koan auth set-role <user> <role>` — change a user's role.
  - `koan auth regenerate-keys` — regenerate Ed25519 keypair, invalidate all tokens.
  - `koan auth reset` — nuclear option, wipe all auth state.
  - Non-interactive setup via `KOAN_USERNAME` + `KOAN_PASSWORD` env vars.
  - Auth enabled by default.
- **1Password CLI integration** — on user creation, offers to generate a secure 32-char password and save to 1Password as `koan@<hostname>`. Updates existing items if found.
- **GraphQL playground with introspection key** — `koan --headless --playground` generates a process-scoped key, injects it into GraphiQL as a default header, auto-opens the browser. Normal JWT auth unaffected.
- **CORS support** — API endpoints accept cross-origin requests for browser clients.

### Security

- Constant-time password comparison in Subsonic API (fixes timing attack).
- Atomic refresh token rotation (single SQL statement, no TOCTOU race).
- Key file permissions set before write (no world-readable window).
- Server panics if auth enabled but keypair missing (fail-closed).
- GraphQL handler requires AuthUser injection (no silent admin fallback).
- Hardcoded admin/admin Subsonic fallback removed.
- Auth keypair directory gets automatic `.gitignore`.

## v0.21.0 (2026-04-12)

### Changed

- **Crate restructure** — split monolithic `koan-music` into four crates with compiler-enforced dependency boundaries. ([#157](https://github.com/radiosilence/koan/issues/157))
  - **koan-core** — audio engine, player, DB, Subsonic client, format strings, config. Platform-agnostic library. Now includes shared helpers (subsonic client builder, cache paths, track resolution, download).
  - **koan-tui** — Ratatui TUI, visualizers, media keys, transport, download queue. Library crate exporting `run_tui()`.
  - **koan-server** — GraphQL (async-graphql + axum), Subsonic REST API, MCP server. Library crate.
  - **koan-cli** — thin entry point with clap CLI, logger, signal handling. Produces the `koan` binary.
  - Dependency rules enforced by Cargo: koan-tui and koan-server cannot import each other. Future iOS app imports only koan-core.

### Added

- **Integration test coverage** — 12 new behavioral tests covering the scanner, decode pipeline, session persistence, remote sync, GraphQL mutations, and config loading. Shared WAV file generators in `test_utils.rs`. Safety net for the crate restructure.
- **Bitrate display for lossy codecs** — transport bar shows bitrate (e.g. `Opus 48kHz/128kbps stereo`) instead of a fake bit depth. Estimated from file size / duration for Opus. ([#155](https://github.com/radiosilence/koan/pull/155))
- **Human-readable quality labels** — `FLAC · CD quality` for 44.1kHz/16bit/stereo, `stereo`/`mono` instead of `2ch`/`1ch`, sample rates as `44.1kHz` not `44100Hz`. ([#155](https://github.com/radiosilence/koan/pull/155))

### Fixed

- **GraphQL connections** — disabled `nodes` shortcut field on Relay connections, edges-only for consistency. ([#166](https://github.com/radiosilence/koan/pull/166))

## v0.20.4 (2026-04-12)

### Fixed

- **Bit depth hidden for lossy codecs** — Opus, Vorbis, AAC, and MP3 no longer show a fake "32bit" in the transport bar. `bit_depth` is now `Option<u16>` — `None` for lossy codecs, displayed only for lossless (FLAC, ALAC, WAV, AIFF). Transport shows `"Opus 48000Hz/2ch"` instead of `"Opus 48000Hz/32bit/2ch"`.

## v0.20.3 (2026-04-12)

### Added

- **Opus codec support** — `.opus` files now play correctly. Uses `opus-decoder` (pure Rust, RFC 8251) to bridge Symphonia's Ogg demuxer with a real Opus decoder. Pre-skip trimming, 48 kHz output, ReplayGain scanning all handled. Closes [#149](https://github.com/radiosilence/koan/issues/149).
- **Secrets-in-git startup check** — on launch, koan checks if config files containing passwords are tracked by git. If so, the app refuses to start and prints remediation steps (remove from git, add to .gitignore, rotate credentials). Hard panic, no bypass.

### Fixed

- **Scan FK constraint error** — `koan scan` failed with `FOREIGN KEY constraint failed` when removing stale tracks that had rows in `lyrics_cache`, `play_history`, or `track_vectors`. Now cleans all FK references before deleting. ([#152](https://github.com/radiosilence/koan/pull/152))
- **Multi-instance state clobber** — autosave is now event-driven (dirty flag) and throttled to 100ms. An idle koan window no longer overwrites the saved state of an active instance.

### Changed

- **Symphonia format support** — added ADPCM codec, MKV/WebM and CAF container support.

## v0.20.2 (2026-04-12)

### Changed

- **Reactive background** — beat-pulsing background color on braille modes (starfield, wormhole, kaleidoscope, lissajous, wireframe, spiral) moved behind `[visualizer] reactive_bg = false` config flag instead of being removed. Off by default.

## v0.20.1 (2026-04-12)

### Fixed

- **Matrix rain character flicker** — characters no longer flash/disappear globally. Each position flickers independently at ~2hz with staggered phase offsets.
- **Matrix rain speed** — reverted post-release speed experiments. Back to the v0.20.0 formula (band energy + beat + bass) with time-based frame_dt scaling so speed is consistent across different FPS targets.
- **Reactive background removed** — the beat-pulsing background color on braille modes (starfield, wormhole, kaleidoscope, etc.) looked flickery. Transparent background integrates better with the TUI.
- **Pleasures layout** — artist/album text properly spaced with blank lines above and below. Waveform box no longer clips peaks at the top (height scale capped per ridgeline). Raised cosine window tapers ridgelines to flat baselines at the edges.
- **Animation timing** — all visualizer animations now use actual frame delta time instead of hardcoded 1/60. Consistent speed at 30fps, 60fps, or 120fps.

### Added

- **Symphonia format support** — added ADPCM codec, MKV/WebM and CAF container support. Opus decoding is not yet supported (see [#149](https://github.com/radiosilence/koan/issues/149)).
- **BPM detection** — beat onset interval tracking with median estimation. Stored on VisualizerState for future use. Resets on track changes.

## v0.20.0 (2026-04-11)

### Added

- **22 visualizer modes** — massively expanded from 5 to 22 modes, cycle with `M` key or use the new picker (`v`). Press `F` for fullscreen. All modes use the palette system, beat-reactive color/hue shifts, and dreamy drift.

  **Analytical:** `spectrogram` (time×frequency heatmap with blue→yellow→red→white heat map, sqrt amplitude scaling), `stereo` (L/R waveforms stacked top/bottom), `vu` (dual analog needle meters with ballistic physics), `flame` (filled spectrum curve with 8 stacked decay trails).

  **Winamp-inspired:** `plasma` (overlapping sine waves, audio-reactive parameters), `tunnel` (polar fly-through with ring/stripe texturing), `wireframe` (3D torus mesh with spectrum-modulated vertices, perspective projection), `metaballs` (6 implicit surface blobs driven by spectrum bands), `starfield` (1500 3D stars with perspective projection, bass-driven speed, motion trails), `pleasures` (pure white ridgelines from spectrum history with raised cosine window, artist/album labels).

  **Psychedelic:** `moire` (three rotating line grids, interference patterns), `kaleidoscope` (8-fold symmetry mirror of spectrum-driven radial patterns), `julia` (Julia fractal with audio-driven complex constant, smooth escape coloring), `spiral` (Archimedean spiral arms modulated by spectrum), `interference` (concentric wave sources, ripple moiré), `wormhole` (3D wireframe tunnel fly-through with procedural geometry, background stars).

  **Special:** `matrix` (authentic cmatrix-style digital rain with katakana characters, per-column spectrum-mapped fall speed, beat-spawned clusters).

- **Visualizer picker modal** — press `v` to open a fullscreen picker. Arrow keys scroll with live preview in the background. Enter confirms, Esc reverts. `M` still cycles directly. ([#147](https://github.com/radiosilence/koan/pull/147))

- **Matrix overlay** — press `X` to toggle. Post-processing pass that replaces all rendered characters with random matrix glyphs in green, preserving the spatial structure. Works on any visualizer mode. Config: `[visualizer] matrix_overlay`. ([#147](https://github.com/radiosilence/koan/pull/147))

- **Bass shake** — camera jitter + scale pulse on bass hits. Applied to braille-rendered modes (oscilloscope, radial, wireframe, starfield, lissajous, wormhole, kaleidoscope, spiral). Press `S` to toggle. Config: `[visualizer] bass_shake = true`. ([#147](https://github.com/radiosilence/koan/pull/147))

- **Reactivity config** — `[visualizer] reactivity` (0.0–2.0, default 1.0). Scales all beat/spectrum-driven animation coefficients. Crank to 2.0 for DnB, dial to 0.3 for ambient. ([#147](https://github.com/radiosilence/koan/pull/147))

- **Beat-reactive backgrounds** — starfield, wormhole, kaleidoscope, lissajous, wireframe, spiral get a subtle pulsing background color that shifts with beat hue offset. ([#147](https://github.com/radiosilence/koan/pull/147))

- **Drag-to-resize transport bar** — click and drag the bottom edge of the transport/album art area to resize it. Makes more room for the visualizer or enlarges album art. Persisted to config. ([#147](https://github.com/radiosilence/koan/pull/147))

### Changed

- **Braille rendering** — all braille cells now rendered bold with +25% brightness boost to compensate for dot sparsity. ([#147](https://github.com/radiosilence/koan/pull/147))
- **Spectrogram** — dedicated heat map colorscale (blue→yellow→red→white) with sqrt amplitude scaling for full dynamic range. No longer uses the palette system. ([#147](https://github.com/radiosilence/koan/pull/147))

## v0.19.5 (2026-04-11)

### Added

- **Four new visualizer modes** — cycle with `M` key through nine total modes. New additions: `spectrogram` (time×frequency heatmap scrolling vertically, block characters for density), `stereo` (L and R waveforms stacked top/bottom with warm/cool palette split), `vu` (dual analog needle meters with arc scale, tick marks, and ballistic needle physics — fast attack, slow decay), `flame` (filled area under the spectrum curve with 8 stacked decay trails creating a layered mountain/fire effect). All modes use the existing palette system, beat-reactive color shifts, and dreamy drift. Config: `[visualizer] mode = "spectrogram"` (or `waterfall`, `stereo`, `vu`, `meter`, `flame`, `mountain`). ([#146](https://github.com/radiosilence/koan/pull/146))

## v0.19.4 (2026-04-11)

### Fixed

- **Braille visualizer modes running at ~11fps instead of 60fps** — the decode thread pushed entire packets to the visualization buffer in one shot, then blocked waiting for the audio ring buffer to drain. For FLAC (4096 frames/packet at 44.1kHz), VizBuffer only got fresh data every ~93ms (~11fps). Spectrum bars hid this with decay smoothing, but waveform-based modes (oscilloscope, lissajous, radial, particles) rendered the same frozen samples 5-6 frames in a row before jumping. VizBuffer writes now happen incrementally inside the ring buffer push loop, paced by the audio callback's real-time consumption rate. All visualizer modes now update at true 60fps.
- **Double-smoothed spectrum bars** — the TUI applied its own decay smoothing on top of the analyzer's, making transients mushier than intended. Spectrum, peaks, and VU levels now pass through directly from the analyzer thread (single layer of smoothing). Beat energy retains local decay for the hue-shift effect.

## v0.19.3 (2026-04-09)

### Fixed

- **Incomplete downloads can't corrupt the cache** — downloads now write to a `.part` file and atomically rename on completion. Interrupted downloads are cleaned up, never mistaken for complete files. Size verification against Content-Length catches server-side truncation. Streaming playback reads from RAM (StreamBuffer) so the rename is invisible to the decoder. ([#143](https://github.com/radiosilence/koan/pull/143))

## v0.19.2 (2026-04-09)

### Fixed

- **Streaming playback fails on restored sessions with unmounted volumes** — when a track's original local path no longer exists (e.g. volume unmounted), the streaming system tried to open the stale path instead of the cache download destination. Now updates the item path to the cache dest before downloading starts. Also checks if the local file came back (volume remounted) before re-downloading. ([#140](https://github.com/radiosilence/koan/pull/140))

## v0.19.1 (2026-04-06)

### Added

- **Braille visualizer modes** — five rendering modes for the visualizer, switchable with `M` key: `bars` (existing LED spectrum), `oscilloscope` (raw PCM waveform as braille line), `radial` (polar-coordinate spectrum starburst), `particles` (frequency-driven particle system with physics), `lissajous` (stereo phase scope with afterglow trail). All modes use a braille character grid (U+2800..U+28FF) for 2x4 subpixel resolution per terminal cell. Beat-reactive, palette-colored, existing color palettes and drift effects apply to all modes. Config: `[visualizer] mode = "bars"` (default). ([#137](https://github.com/radiosilence/koan/issues/137))

## v0.19.0 (2026-04-06)

### Added

- **Colorful spectrum analyzer** — frequency-mapped rainbow replaces monochrome green. Four palettes via `[visualizer] palette`: `spectrum` (default), `fire`, `neon`, `mono`. Dreamy 8-second color drift breathes the rainbow back and forth across the bars. Beat-reactive hue shifts snap the palette forward on kicks/transients. Brightness pulses on top. Peak markers glow in brightened palette colors. All color math in the render path — zero impact on audio threads. ([#134](https://github.com/radiosilence/koan/issues/134), [#135](https://github.com/radiosilence/koan/pull/135))

## v0.18.7 (2026-04-05)

### Changed

- **Sample rate switching uses CoreAudio property listener instead of polling** — `set_device_sample_rate` now registers an `AudioObjectAddPropertyListener` on `kAudioDevicePropertyNominalSampleRate` and blocks on a oneshot channel instead of spinning every 10ms. Eliminates up to 10ms unnecessary latency per rate switch. Timeout bumped from 2s to 5s to cover USB Class 1 DACs doing PLL relock. Early-out when rate already matches, spurious callback verification, RAII listener cleanup. ([#130](https://github.com/radiosilence/koan/issues/130))

## v0.18.6 (2026-04-05)

### Fixed

- **`koan play /dir` with large libraries** — for >1000 files, uses a single `all_tracks_by_path` DB query instead of hundreds of batched `WHERE IN` queries. Directory walk + metadata resolution now runs on a background thread so the TUI starts immediately ([#128](https://github.com/radiosilence/koan/pull/128))
- **Organize preview takes minutes on large libraries** — `preview_for_paths` now loads metadata from the DB (single query) instead of re-reading every file's tags from disk. Falls back to parallel disk reads (rayon) for files not in the DB. 48k-track library: ~5 minutes → ~3 seconds ([#128](https://github.com/radiosilence/koan/pull/128))

## v0.18.5 (2026-04-05)

### Fixed

- **Hi-res audio playing at wrong speed** — CoreAudio sample rate switches are asynchronous, but the player read back the device rate immediately after requesting the change and got the *old* rate. A fallback (`unwrap_or(source_rate)`) then masked the mismatch by lying to the ASBD. Result: 96kHz files played at quarter speed (device still clocked at the old rate, draining the ring buffer too slowly). `set_device_sample_rate` now polls until CoreAudio confirms the switch (10ms intervals, 2s timeout) and returns the verified rate. Both file and streaming playback paths fixed. ([#124](https://github.com/radiosilence/koan/pull/124))

## v0.18.4 (2026-04-01)

### Fixed

- **Audio seize-up at album transitions** — when the gapless decode loop exhausted the playlist, the Player had no way to know playback finished. Audio engine kept running, outputting silence. Now the decode thread signals `DecodeFinished` and the Player auto-advances or stops cleanly ([#122](https://github.com/radiosilence/koan/pull/122))
- **Double engine restart at session restore** — startup sent Play+Pause+Seek, causing three engine teardown/rebuild cycles. Now sets cursor without playback; the deferred seek is the single start point ([#122](https://github.com/radiosilence/koan/pull/122))
- **Key repeat rapid-skipping** — terminal key repeat on `>`/`<` could fire dozens of NextTrack/PrevTrack commands. Added 150ms debounce in the Player command loop ([#122](https://github.com/radiosilence/koan/pull/122))

### Changed

- **Now-playing queue indicator** — playing track now shows ▶ instead of `>`, with bold title text for visibility ([#122](https://github.com/radiosilence/koan/pull/122))

## v0.18.3 (2026-04-01)

### Changed

- **CLI: `koan init` → `koan config init`** — config initialization is now a subcommand of `koan config`. `koan config` with no subcommand still shows resolved config ([#120](https://github.com/radiosilence/koan/pull/120))
- **Config init generates commented template** — `config.toml` now contains all defaults as commented lines for reference. Uncomment what you want to customize. No more silent duplication of values across config files ([#120](https://github.com/radiosilence/koan/pull/120))
- **`[library]` and `[remote]` excluded from `config.toml`** — machine-specific paths and credentials belong in `config.local.toml` only. Prevents accidental credential leaks into dotfile repos ([#120](https://github.com/radiosilence/koan/pull/120))
- **ReplayGain default changed to `off`** — was `album`. Users who want loudness normalization can opt in via `replaygain = "album"` or `"track"` ([#120](https://github.com/radiosilence/koan/pull/120))

### Fixed

- **`koan remote login` no longer bloats `config.local.toml`** — previously wrote all default config sections; now patches only the `[remote]` section, preserving the rest of the file as-is ([#120](https://github.com/radiosilence/koan/pull/120))
- **Removed `Config::save()` footgun** — method could leak secrets from merged config into `config.toml`. Replaced with `Config::patch_local(section, values)` for targeted local config updates ([#120](https://github.com/radiosilence/koan/pull/120))
- **`.gitignore` now covers `*.db-wal` and `*.db-shm`** — SQLite WAL files were previously not gitignored ([#120](https://github.com/radiosilence/koan/pull/120))

## v0.18.2 (2026-03-29)

### Changed

- **CLI: `koan play` subcommand** — play-related args (`paths`, `--album`, `--artist`, `--id`, `--library`, `--clear`, `--server`, `--jukebox`) moved from the root command to `koan play`. Running bare `koan` still launches the TUI. This fixes zsh tab completions which broke when positional paths were on the root struct alongside subcommands ([#116](https://github.com/radiosilence/koan/issues/116))

### Fixed

- **Tab completions** — zsh/bash/fish completions now correctly suggest subcommands instead of filesystem paths ([#116](https://github.com/radiosilence/koan/issues/116))
- **Docs: `koan mcp`** — corrected all references from `--mcp` flag to `mcp` subcommand
- **Docs: removed fabricated Docker content** — no Docker image exists
- **Docs: GraphQL operations table** — fixed naming convention, added missing operations
- **Docs: radio mode scoring** — corrected signal descriptions to match actual implementation
- **Docs: added missing CLI commands** — `koan analyze`, `koan completions`, `scan --force`
- **Docs: added missing `V` keybinding** for visualizer toggle

- **Documentation rewrite** — slimmed README from 740 lines to a focused hook + install + quickstart + feature list + doc links. All detailed content moved to dedicated guides and references under `docs/`:
  - `docs/getting-started.md` — progressive first-time setup tutorial
  - `docs/guide/` — radio mode, remote servers, file organization, GraphQL API, MCP integration, headless server
  - `docs/reference/` — configuration (all fields including previously undocumented `ticker_fps`, `target_fps`, `show_fps`, `art_size`, `output_device`), keybindings (every key in every mode), CLI reference
  - `docs/recipes/` — troubleshooting, cache management

## v0.18.1 (2026-03-28)

### Changed

- **Config loading uses figment** — replaced hand-rolled TOML deep-merge with [figment](https://docs.rs/figment) for layered config: defaults → `config.toml` → `config.local.toml` → `KOAN_*` env vars. Any config field is now overridable via environment variables using `KOAN_SECTION__FIELD` naming (e.g. `KOAN_REMOTE__PASSWORD`, `KOAN_GRAPHQL__PORT`, `KOAN_PLAYBACK__TARGET_FPS`)

### Fixed

- **Secret round-trip leak in config save** — `save()` on a merged Config would serialize secrets from `config.local.toml` and env vars back into `config.toml`. Callers now use `Config::update_base()` which reads only `config.toml`, applies the mutation, and writes back without leaking sensitive fields

- **Path traversal in organize** — `sanitize_relative_path` now strips `..` and `.` components, and `plan_single_move` validates the destination stays under the base directory. Prevents malicious metadata from writing files outside the library ([#99](https://github.com/radiosilence/koan/issues/99))
- **RT safety in CPAL audio callback** — changed `Mutex::lock()` to `try_lock()` in the audio render callback so the real-time thread never blocks; outputs silence on contention instead ([#99](https://github.com/radiosilence/koan/issues/99))
- **O(N) LRU cache query** — replaced correlated `SELECT MAX(played_at)` subquery per track with a single `LEFT JOIN` on pre-aggregated play_history ([#99](https://github.com/radiosilence/koan/issues/99))
- **Sequential scan_cache lookups** — scanner now batch-loads the entire scan cache into a HashMap instead of issuing one DB query per file, dramatically faster for large libraries ([#99](https://github.com/radiosilence/koan/issues/99))
- **Memory usage on playlist build** — `playlist_items_from_paths` now uses `tracks_by_paths()` (batched IN-query) instead of loading every track in the library into a HashMap ([#99](https://github.com/radiosilence/koan/issues/99))

### Added

- **API concurrency limit** — GraphQL server now applies a tower `ConcurrencyLimitLayer` (max 10 concurrent requests) to prevent mutation spam / DoS ([#99](https://github.com/radiosilence/koan/issues/99))
- **Composite index** on `tracks(album_id, disc, track_number)` for faster album-ordered queries ([#99](https://github.com/radiosilence/koan/issues/99))
- **CoreAudio crash during sample rate switch** — `stop_engine()` was dropping the `AudioEngine` on a background cleanup thread while the player thread immediately changed the device sample rate. The engine is now dropped synchronously before any sample rate changes; only the decode handle cleanup runs in the background ([#89](https://github.com/radiosilence/koan/issues/89))
- **Render callback drain on AudioEngine drop** — `AudioOutputUnitStop` can return before the render callback finishes during sample rate switches. Added `in_callback` atomic flag and spin-wait in `Drop` to ensure the callback has fully exited before tearing down buffers ([#89](https://github.com/radiosilence/koan/issues/89))
- **Pending items never downloaded on session restore** — the cache verify fix correctly marked missing files as `Pending`, but never actually triggered downloads. Introduced a persistent `DownloadQueue` that lives for the app's lifetime: session restore feeds pending items into it, and double-clicking a pending track triggers a priority download with stream-when-ready playback. The same queue replaces the one-shot scoped thread pool previously used by `enqueue_playlist` ([#94](https://github.com/radiosilence/koan/issues/94))
- **GraphQL/Subsonic port bind panic** — `run_api_blocking` called `.expect()` on port bind, crashing the entire app on `AddrInUse`. Now logs a warning and gracefully disables the API server ([#95](https://github.com/radiosilence/koan/issues/95))
- **TUI layout jump when album art loads** — the transport bar now always reserves a 24×12 cell placeholder for album art, preventing layout reflow when art loads or when switching between tracks with/without embedded art ([#96](https://github.com/radiosilence/koan/issues/96))

### Added

- **Track `db_id` in playlist items** — `PlaylistItem` and `PersistedQueueItem` now carry `db_id: Option<i64>`, enabling re-download of remote tracks after session restore. Backwards-compatible: old persisted state without `db_id` deserializes cleanly via `#[serde(default)]` ([#94](https://github.com/radiosilence/koan/issues/94))
- **Cache management with LRU eviction** — cached remote downloads are now tracked in the DB (path, size, download date). Set `cache_limit` in `[remote]` config (e.g. `"50GB"`) to enable automatic LRU eviction on startup. Evicts whole albums, oldest last-played first. Favourited tracks are never evicted. New `koan cache evict` subcommand for manual eviction ([#88](https://github.com/radiosilence/koan/issues/88))

## v0.17.1 (2026-03-27)

### Fixed

- **GraphQL/Subsonic servers now bind to 127.0.0.1 by default** — previously bound to `0.0.0.0` with no authentication, exposing library enumeration, file moves, and queue clearing to anyone on the network. Added `bind` field to `[graphql]` config and `--bind` CLI flag ([#85](https://github.com/radiosilence/koan/issues/85))

### Added

- **Album-aware download priority** — when a track starts playing, remaining tracks from the same album are bumped to the front of the download queue, ensuring gapless album playback ([#87](https://github.com/radiosilence/koan/issues/87))
- **CONTRIBUTING.md** — contribution guidelines ([#82](https://github.com/radiosilence/koan/issues/82))

## v0.17.0 (2026-03-26)

### Fixed

#### Remote

- **Replace hand-rolled ISO 8601 parser with chrono** — the manual RFC 3339 parser in `remote/sync.rs` (~70 lines) could panic on malformed input from Subsonic servers. Replaced with `chrono::DateTime::parse_from_rfc3339()` + fallback patterns for common server quirks (missing timezone, fractional seconds, space separators). Added 11 unit tests ([#74](https://github.com/radiosilence/koan/pull/74))

#### Audio

- **Atomic ordering hardened** — `samples_played` uses `AcqRel` on fetch_add, `Acquire` on loads. `running` flag uses `Acquire`/`Release`. No more `Relaxed` for cross-thread state ([#76](https://github.com/radiosilence/koan/pull/76))
- **Timeline lock ordering** — `PlaybackTimeline::current_playback()` now acquires the boundaries read lock first, then reads `samples_played` inside that scope. Dead standalone `channels`/`sample_rate` atomics removed ([#76](https://github.com/radiosilence/koan/pull/76))
- **Alignment check in CoreAudio callback** — replaced `debug_assert!` with a runtime check that fills silence on misalignment instead of UB ([#76](https://github.com/radiosilence/koan/pull/76))
- **Buffer bounds validation** — `ptr::copy_nonoverlapping` in `engine.rs` and `cpal_backend.rs` now clamps to available space and fills remainder with silence ([#76](https://github.com/radiosilence/koan/pull/76))
- **VizBuffer allocation reuse** — added `VizBuffer::snapshot_into(&self, out: &mut Vec<f32>)` to reuse caller's buffer instead of allocating per frame ([#76](https://github.com/radiosilence/koan/pull/76))

#### Player

- **Atomic ordering across player state** — all cross-thread atomics (`playback_state`, `position_ms`, `playback_generation`, `playlist_version`, `bytes_written`, `quit_requested`, `metadata_refresh_pending`, `radio_mode`, `pump_written`) upgraded from `Relaxed` to `Acquire`/`Release`/`AcqRel` ([#78](https://github.com/radiosilence/koan/pull/78))
- **Undo stack O(1) eviction** — `Vec` replaced with `VecDeque` for O(1) `pop_front()` instead of O(n) `remove(0)`. Batch depth capped at 500 to prevent unbounded nesting ([#78](https://github.com/radiosilence/koan/pull/78))
- **Seek underflow on short tracks** — guard `max_ms > 5_000` before subtracting safety margin, preventing short/partially-downloaded tracks from clamping seek to 0 ([#78](https://github.com/radiosilence/koan/pull/78))
- **ClearPlaylist snapshot race** — `stop_playback_and_clear_state()` (engine teardown only) now runs before snapshotting the playlist for undo, then clears. Previously `stop()` cleared the playlist before the undo snapshot was captured ([#78](https://github.com/radiosilence/koan/pull/78))

#### TUI

- **Terminal restoration on any thread panic** — removed main-thread-only guard from panic hook. Terminal is restored (raw mode, alternate screen, mouse capture, bracketed paste, cursor) regardless of which thread panics ([#77](https://github.com/radiosilence/koan/pull/77))
- **Cursor clamping on queue mutation** — `clamp_queue_cursor()` called after `delete_selected()` and on every playlist version change. Render-time clamp kept as safety net ([#77](https://github.com/radiosilence/koan/pull/77))
- **Cover art cache bounded** — `CoverArt::clear()` frees the `DynamicImage` when nothing is playing ([#77](https://github.com/radiosilence/koan/pull/77))
- **Double-click timeout clearing** — stale `last_click_time`/`last_click_idx` cleared after 1 second, preventing misinterpreted double-clicks ([#77](https://github.com/radiosilence/koan/pull/77))
- **Picker cursor safety** — render loop clamps cursor to `matched_count` range before computing scroll offset, preventing out-of-bounds when results shrink between ticks ([#77](https://github.com/radiosilence/koan/pull/77))

#### Database

- **Transaction boundaries** — `upsert_track()` wraps the entire artist/album/track/FTS5 operation in a savepoint. Scanner and analyzer use proper `unchecked_transaction()` with error propagation instead of silent `let _ =` drops ([#79](https://github.com/radiosilence/koan/pull/79))
- **Source column validation** — `CHECK (source IN ('local', 'remote', 'cached'))` constraint on tracks table ([#79](https://github.com/radiosilence/koan/pull/79))
- **WAL checkpoint on connect** — `PRAGMA wal_checkpoint(PASSIVE)` at connection open prevents unbounded WAL growth across sessions ([#79](https://github.com/radiosilence/koan/pull/79))
- **LIKE wildcard escaping** — `remove_stale_tracks` now escapes `%`, `_`, `\` in path prefixes via `escape_like()` ([#79](https://github.com/radiosilence/koan/pull/79))
- **Missing index** — added `idx_library_folders_path` on `library_folders(path)` ([#79](https://github.com/radiosilence/koan/pull/79))

#### Misc

- **Batch ID collision** — `chrono_batch_id()` uses `as_nanos()` instead of `as_millis()` to prevent millisecond-resolution collisions ([#75](https://github.com/radiosilence/koan/pull/75))
- **Unicode-aware string comparison** — `stricmp` format function uses `.to_lowercase()` instead of ASCII-only `eq_ignore_ascii_case()` ([#75](https://github.com/radiosilence/koan/pull/75))
- **Ancillary file move errors logged** — `execute_single_move_no_db` now logs warnings via `log::warn!` instead of silently swallowing with `.ok()` ([#75](https://github.com/radiosilence/koan/pull/75))
- **Tokio features scoped** — `"full"` replaced with `["rt-multi-thread", "net", "macros", "signal"]` in koan-music ([#75](https://github.com/radiosilence/koan/pull/75))

### Changed

- **Unified daemon mode** — `koan` now runs TUI + GraphQL API in one process by default. No more separate `koan serve`. All interfaces share one player, one state ([#70](https://github.com/radiosilence/koan/issues/70))
  - `koan --headless` replaces `koan serve` (GraphQL API only, no TUI)
  - `koan -d` / `koan --daemonize` forks a headless background daemon
  - `koan --mcp` replaces `koan mcp` (MCP server on stdio)
  - `koan --no-api` opts out of the API server (TUI-only, old behaviour)
  - `koan --port`, `--subsonic`, `--playground` configure the API from top-level
  - Play args (`--album`, `--artist`, `--id`, `--library`, `--clear`, `--server`, `--jukebox`) moved to top-level — `koan play` removed
  - `koan scan --analyze` combines scan + acoustic analysis in one pass

### Removed

- **`koan play`** — `koan` IS play. All args moved to top-level
- **`koan serve`** — replaced by `koan --headless`
- **`koan graphql`** — dead alias, removed
- **`koan mcp`** — replaced by `koan --mcp` flag
- **`koan pick`** — standalone picker removed, TUI has built-in pickers (`p`/`a`/`r`)
- **`koan artists`**, **`koan albums`** — use `koan search` or GraphQL queries

## 0.16.0

### Changed

- **GraphiQL v2** — replaced deprecated GraphQL Playground with the official GraphQL Foundation IDE. Actively maintained, subscription-ready, better UX. `koan serve --playground` now serves GraphiQL ([#71](https://github.com/radiosilence/koan/issues/71))
- **Clean schema type names** — stripped `Gql` prefix from all GraphQL types. `GqlArtist` → `Artist`, `GqlTrack` → `Track`, `GqlNowPlaying` → `NowPlaying`, etc. The public schema now has clean, idiomatic names

## 0.15.0

### Added

- **Linux audio support** — `AudioBackend` trait abstraction with `CpalBackend` (ALSA/PipeWire/PulseAudio via cpal) for Linux and `CoreAudioBackend` for macOS. Bit-perfect gapless on both platforms. Decode pipeline untouched — backends are dumb ring buffer consumers ([#58](https://github.com/radiosilence/koan/pull/58))
- **`koan serve`** — unified server command. GraphQL API (always on) + optional Subsonic REST (`--subsonic <port>`). Replaces `koan graphql`. One process, one player, two interfaces ([#55](https://github.com/radiosilence/koan/pull/55))
- **Subsonic REST API** — 22 endpoints for third-party clients (play:Sub, Amperfy). Browsing, search, streaming with Range + proxy, cover art, star/unstar, scrobble, playlists (mapped to snapshots), genres. XML + JSON, MD5+salt auth ([#55](https://github.com/radiosilence/koan/pull/55))
- **`koan play --server`** — TUI client mode via GQL. Streams audio locally from a remote `koan serve` instance ([#55](https://github.com/radiosilence/koan/pull/55))
- **`--jukebox` mode** — server plays audio, client is remote control only ([#55](https://github.com/radiosilence/koan/pull/55))
- **Acoustic similarity** — `koan analyze` generates 23-dim bliss-audio fingerprints. Radio mode gains `SimilarityAxis::Acoustic`. `similarTracks(trackId, limit)` GQL query ([#68](https://github.com/radiosilence/koan/pull/68))
- **GraphQL API** — full Relay-style cursor pagination, rich metadata filters (year, codec, genre, sample rate, bit depth, duration), fuzzy search, lyrics, cover art, organize, scan, sync, share mutations ([#36](https://github.com/radiosilence/koan/pull/36))
- **MCP server: GraphQL-first** — 2 tools: `schema_sdl` + `graphql`. Claude reads the schema, drives everything through one tool
- **Named queue snapshots** — save/restore/list/delete via GQL + MCP. Bank curated mixes and switch between them
- **Radio mode via API** — `enableRadio`/`disableRadio` mutations. SharedPlayerState atomic keeps TUI and API in sync
- **Favourites filter + remote sync** — `favouritesOnly` on all queries, `isFavourite` on tracks. Star/unstar auto-syncs to Subsonic/Navidrome
- **`[discovery]` config** — `analysis_on_scan`, `acoustic_weight` for acoustic similarity tuning
- **Neural discovery (feature-gated)** — DCLAP ONNX embeddings behind `neural-discovery` cargo feature. `textSearch` GQL query, `koan analyze --neural`. Opt-in, graceful degradation ([#69](https://github.com/radiosilence/koan/pull/69))
- **Cross-platform credentials** — `keyring` crate replaces `security-framework` (macOS Keychain + Linux secret-service)
- **CI for Linux** — clippy, test, build on macOS + Ubuntu. Release binaries: macOS arm64/x86_64 + Linux x86_64/arm64 (native runners)

### Fixed

- **Remote tracks silently skipped** — GQL mutations now trigger background downloads. Correct cache paths via `resolve_item_path()` (single code path with TUI)
- **`restoreSnapshot` downloads** — snapshot restore now runs the download pipeline like `addToQueue`
- **N+1 query elimination** — genre/favourite filtering uses batch SQL instead of per-item calls ([#64](https://github.com/radiosilence/koan/pull/64))
- **GraphQL injection** — query building converted from `format!()` to proper variables ([#63](https://github.com/radiosilence/koan/pull/63))
- **Remote bridge hardening** — exhaustive `PlayerCommand` match, incomplete downloads marked Failed, 30s HTTP timeouts ([#60](https://github.com/radiosilence/koan/pull/60))
- **Linux: ALSA/JACK stderr spam** — cpal backend probe output suppressed via fd redirect during all operations
- **Linux: Ctrl+C terminal restore** — second Ctrl+C force-restores raw mode and exits immediately
- **Scanner: empty files** — 0-byte files get clear error instead of confusing Symphonia probe messages
- **`--playground` flag** — changed from `Option<bool>` to proper flag
- **`insert_in_queue`** — was silently appending, now uses `InsertInPlaylist`
- **Ctrl+C on GQL server** — graceful shutdown via `tokio::signal::ctrl_c`

### Changed

- **graphql.rs split** — 2400-line file decomposed into `graphql/{mod,types,queries,mutations,helpers,server}.rs` ([#67](https://github.com/radiosilence/koan/pull/67))
- **`Player` holds `Box<dyn AudioBackend>`** — all device/engine calls go through trait
- **SubsonicClient factory** — `subsonic_client()` helper replaces 9 manual creation sites ([#65](https://github.com/radiosilence/koan/pull/65))
- **Player device restart dedup** — `restart_on_current_track()` + `Config::load_or_default()` ([#62](https://github.com/radiosilence/koan/pull/62))
- **serve.rs route dedup** — `register_subsonic_routes()` shared between prod and test ([#61](https://github.com/radiosilence/koan/pull/61))
- **Platform-gated deps** — `coreaudio-sys`/`core-foundation` macOS-only, `cpal` Linux-only

## 0.14.0

### Added

- **Acoustic similarity** — `koan analyze` generates 23-dim acoustic fingerprints (tempo, timbre, chroma, spectral features) via bliss-audio. Stored in SQLite, brute-force KNN is sub-millisecond. Radio mode gains `SimilarityAxis::Acoustic` — finds tracks that *sound* similar regardless of metadata. `similarTracks(trackId, limit)` GraphQL query for "more like this"
- **`[discovery]` config section** — `analysis_on_scan` (run analysis during library scan, default false) and `acoustic_weight` (scoring weight for acoustic signal)
- **Empty file handling** — scanner skips 0-byte files with a clear error instead of confusing "probe reach EOF" messages

## 0.13.1

### Fixed

- **N+1 query elimination** — genre and favourite filtering now use batch SQL queries instead of per-item DB calls. O(1) instead of O(n*m) on large libraries
- **Remote bridge hardening** — exhaustive PlayerCommand match (compiler catches new variants), incomplete downloads marked as Failed instead of Ready, 30s HTTP timeouts
- **GraphQL client injection fix** — all query building converted from format!() string interpolation to proper GraphQL variables
- **Player device restart dedup** — extracted shared restart logic, config load errors now logged instead of silently swallowed (`Config::load_or_default()`)
- **SubsonicClient factory** — single `subsonic_client()` helper replaces 9 manual construction sites, 30s timeout on all HTTP clients
- **serve.rs route dedup** — extracted `register_subsonic_routes()`, test router no longer duplicates prod routes
- **CI reliability** — arm64 cross-compile no longer silently fails, tags not force-pushed, doc tests added

### Changed

- **graphql.rs split** — 2400-line god file decomposed into `graphql/{mod,types,queries,mutations,helpers,server}.rs`

## 0.13.0

### Added

- **Linux audio support** — `AudioBackend` trait abstraction with `CpalBackend` (ALSA/PipeWire/PulseAudio via cpal) for Linux and `CoreAudioBackend` wrapper for macOS. Bit-perfect gapless playback on both platforms. The decode pipeline and ring buffer are untouched — backends are dumb consumers
- **`koan serve`** — unified server command. GraphQL API (always on) + optional Subsonic REST (`--subsonic <port>`). Replaces `koan graphql` (kept as hidden alias). One process, one player, two interfaces
- **Subsonic REST API** — 22 endpoints for third-party client compatibility (play:Sub, Amperfy). Browsing, search, streaming with Range support, cover art, star/unstar, scrobble, playlists (mapped to snapshots), genres. XML + JSON, MD5+salt auth. Proxy streaming for remote tracks
- **`koan play --server`** — TUI client mode. Connects to a remote `koan serve` via GQL. Client streams audio locally from the server
- **`--jukebox` mode** — server plays audio, client is remote control only
- **GQL client library** in koan-core — typed helpers for all queries and mutations
- **Cross-platform credentials** — `keyring` crate replaces `security-framework`. macOS Keychain + Linux secret-service
- **CI builds for Linux** — clippy, test, build on both macOS and Ubuntu. Release binaries for macOS arm64/x86_64 + Linux x86_64/arm64

### Changed

- `Player` holds `Box<dyn AudioBackend>` instead of direct CoreAudio FFI
- Platform-gated deps: `coreaudio-sys`/`core-foundation` macOS-only, `cpal` Linux-only

## 0.12.5

### Fixed

- **Remote tracks now play when queued via GQL/MCP** — two-part fix:
  1. GQL mutations now trigger background downloads for remote tracks (0.12.4)
  2. Remote tracks now get the correct cache path via `resolve_item_path()` — same code path as the TUI

## 0.12.4

### Fixed

- **Remote track download pipeline wired to GQL mutations** — `addToQueue` and `replaceQueue` now spawn background downloads for remote tracks using the same pipeline as the TUI

## 0.12.3

### Added

- **`lyrics(trackId)` query** — fetch synced LRC or plain text lyrics for any track. Checks embedded tags, sidecar `.lrc` files, and LRCLIB. Cached in DB
- **`coverArt(trackId)` query** — extract embedded cover art as base64 with MIME type. Supports JPEG and PNG
- **`organizePreview` / `organizeExecute` mutations** — preview and execute file renames using fb2k-compatible format strings. Supports per-track or whole-library operations
- **`organizeUndo` mutation** — undo the last organize batch
- **`triggerScan` mutation** — trigger a library rescan from the API. Returns added/updated/unchanged counts
- **`triggerRemoteSync` mutation** — trigger Subsonic/Navidrome library sync from the API
- **`createShare(trackIds, description)` mutation** — create Subsonic sharing links for tracks. Returns the public URL. Claude can now share what it's playing

## 0.12.2

### Added

- **`fuzzySearch` GraphQL query** — nucleo-powered typo-tolerant fuzzy matching for tracks, albums, and artists. Same engine as the TUI picker (and Helix editor). Returns ranked results. `{ fuzzySearch(query: "aphx twn", kind: TRACK, limit: 10) { id name rank kind } }`

## 0.12.1

### Changed

- **MCP server: GraphQL-first interface** — stripped 40+ individual tools down to just 2: `schema_sdl` (introspect the full schema) and `graphql` (execute any query or mutation). All operations go through GraphQL now. Claude calls `schema_sdl` first to learn the API, then uses `graphql` for everything. Cleaner, less context overhead, same capabilities

## 0.12.0

### Added

- **GraphQL API** — `koan graphql` starts a headless player with an HTTP GraphQL server (default port 4000). Full Relay-style cursor pagination on artists, albums, and tracks. One nested query replaces multiple MCP tool calls: `{ artists(first: 100) { edges { node { id, name } } } }`. Mutations for all playback control, queue management, favourites, and device switching. Optional GraphQL Playground UI at `GET /graphql` with `--playground` flag or `playground = true` in `[graphql]` config
- **MCP `graphql` tool** — single tool on the MCP server that executes GraphQL queries in-process (no HTTP). Claude Desktop can now fetch artists, albums, and tracks with nested queries in one round-trip instead of fanning out across individual tools
- **`[graphql]` config section** — `port` (default 4000) and `playground` (default false) in config.toml
- **Named queue snapshots** — save/restore/list/delete named queue states via GQL mutations (`saveSnapshot`, `restoreSnapshot`, `deleteSnapshot`) and MCP tools (`save_snapshot`, `restore_snapshot`, `list_snapshots`, `delete_snapshot`). Bank the techno, switch to hardcore, jump back. Stored in the DB (`queue_snapshots` table) with queue JSON, cursor path, and playback position
- **Radio mode via API** — `enableRadio`/`disableRadio` GQL mutations and `enable_radio`/`disable_radio`/`radio_status` MCP tools. Radio mode was previously TUI-only (Shift+R). Uses SharedPlayerState atomic so TUI and API stay in sync
- **Favourites filter** — `favouritesOnly: true` parameter on `artists`, `albums`, and `tracks` queries. Dedicated `favourites` query with cursor pagination. `isFavourite` field on track type
- **Favourite → remote sync** — favouriting/unfavouriting via GQL or MCP automatically syncs to Subsonic/Navidrome (`star`/`unstar` API) on a background thread. Fire-and-forget, best-effort
- **`clear_device` MCP tool** — reset audio output to system default (was GQL-only)
- **Daemon mode** — `koan graphql -d` forks the server into background, writes PID to `~/.config/koan/graphql.pid`. Claude Code can start it and query via HTTP
- **`schema_sdl` MCP tool** — dumps the full GraphQL schema in SDL format so Claude can introspect all available queries, mutations, types, and filter params on first connect
- **`similarArtists` query** — returns scored similar artists (from ListenBrainz, MusicBrainz, Subsonic) with source and relationship type
- **`playHistory` query** — recent play history with track info, paginated
- **Comprehensive MCP instructions** — rewritten server instructions guide Claude through discovery, the graphql power tool, all filter params, snapshots, radio, favourites, and device control
- **Rich metadata filters on all queries** — albums: `title`, `yearStart`/`yearEnd`, `codec`, `label`, `genre`. Tracks: `title`, `artistName`, `albumTitle`, `genre`, `codec`, `yearStart`/`yearEnd`, `minSampleRate`, `minBitDepth`, `channels`, `minDurationMs`/`maxDurationMs`. Artists: `genre`. All string filters case-insensitive substring

### Fixed

- **`insert_in_queue` MCP tool** — was silently appending instead of inserting after the specified `after_queue_item_id`. Now uses `InsertInPlaylist` command directly
- **`--playground` CLI flag** — was `Option<bool>` requiring `--playground true`. Now a proper flag
- **Ctrl+C on GraphQL server** — `axum::serve` was blocking forever. Added `with_graceful_shutdown` using `tokio::signal::ctrl_c`

## 0.11.1

### Fixed

- **MCP server crash on startup** — all tool return types now use object schemas (MCP 2025-11-25 spec requires `outputSchema` root type to be `object`). Bare string and array returns replaced with `StatusResponse`, `QueueResponse`, `TrackListResponse`, `ArtistListResponse`, `AlbumListResponse`, `DeviceListResponse` wrapper types

### Tests

- **32 MCP server tests** — coverage for all playback commands, queue management (add/remove/clear/replace/reorder), library discovery (search, list_artists, list_albums, list_tracks, get_track, library_stats), state queries (now_playing, list_devices, set_device), UUID parsing, track resolution, and error paths

## 0.11.0

### Added

- **MCP server** — `koan mcp` runs koan as a headless MCP (Model Context Protocol) server on stdio, controllable by Claude Desktop or any MCP client. Exposes 21 tools: playback control (play/pause/resume/stop/next/previous/seek), queue management (add/insert/remove/clear/replace/reorder/get), library discovery (search/list_artists/list_albums/list_tracks/get_track/library_stats), state queries (now_playing/list_devices/set_device), and favourites (favourite/unfavourite/list_favourites). The LLM provides the taste and reasoning — koan just exposes the controls. Configure in Claude Desktop with `{"command": "koan", "args": ["mcp"]}`
- **Visualiser toggle** — press `V` (Shift-V) to enable/disable the spectrum visualiser at runtime. Persists to config.toml. Visible in `?` help menu under Toggles
- **Multi-signal radio mode** — radio now uses ListenBrainz ML similarity, MusicBrainz relationship graph (collaborators, band members, associated acts), Subsonic, genre/era matching, and play history to pick tracks across multiple axes instead of just one source. Drifting seed window follows your recent plays instead of anchoring to a single track. Recency scoring surfaces buried gems (never-played and long-forgotten tracks get a discovery bonus). New config options: `history_window` (don't repeat last N, default 200), `seed_window` (last N plays as seed, default 5), `discovery_weight` (0.0-1.0, default 0.3)
- **Play history tracking** — koan now records track completions in a `play_history` table, used for recency scoring in radio mode and future scrobbling

## 0.10.0

### Added

- **Radio mode** — press `R` to toggle infinite play. When the queue runs low, koan automatically picks similar tracks using Subsonic `getSimilarSongs2` (when a remote server is configured), cached similar-artist relationships, and genre/artist matching from the local library. A magenta `RADIO` badge appears in the hint bar when active. Configurable via `[radio]` in config.toml (lookahead, batch_size, use_subsonic)

## 0.9.2

### Fixed

- **UI freeze on track change** — `stop_engine()` no longer blocks the player command loop waiting for the decode thread to join. Engine teardown (thread join + AudioUnit dispose) is moved to a background cleanup thread, so the player stays responsive even when CoreAudio or I/O is slow to shut down
- **Escape sequence dump on crash** — the panic hook was calling `disable_raw_mode()` and `LeaveAlternateScreen` from whichever thread panicked, corrupting the terminal when a background thread (decode, download) hit an error. The hook now captures the main thread ID at install time and only restores terminal state from the TUI thread
- **Decode thread panic on missing file** — `SourceEntry::from_file` used `panic!` when a file couldn't be opened (e.g. deleted during gapless lookahead). The `make_mss` closure is now fallible (`-> io::Result`), and decode errors are logged gracefully instead of crashing
- **AudioEngine drop race** — removed unreliable `thread::yield_now()` before `AudioUnitUninitialize`. `AudioOutputUnitStop` is synchronous (callback guaranteed finished on return), so no extra wait is needed. The callback is also explicitly removed as a safety net for the rare case where stop fails
- **Silent decode thread panics** — `DecodeHandle::stop()` now logs the panic message instead of silently swallowing `handle.join()` errors

## 0.9.1

### Added

- **Queue persistence** — queue and playback position are automatically saved every second and restored on next launch. Ctrl+C and `q` both trigger a clean save. Use `--clear` to start fresh instead of restoring
- **Graceful Ctrl+C** — replaced raw `SIG_DFL` with a safe signal handler so Ctrl+C performs a clean shutdown (saving state, restoring terminal) instead of killing the process

### Fixed

- **Quit race condition** — quit handlers were sending `PlayerCommand::Stop` (which clears the playlist) before saving state, so persisted queue was always empty. Stop is now sent after saving

## 0.9.0

### Added

- **Navidrome share links** — right-click a song or album header in the queue and select "Share link" to create a public sharing URL via the Subsonic API. The link is copied to clipboard and shown in the hint bar. Prefers album-level shares when all selected tracks are from the same album. Requires `[remote]` to be configured with sharing enabled on the server
- **Double-click album headers** — double-clicking an album header in the queue now starts playback from the first track of that album

## 0.8.2

### Added

- **Homebrew tap** — `brew install radiosilence/koan/koan`. Formula auto-updates on each release via CI

## 0.8.1

### Added

- **Help modal** — press `?` to open a two-column keybindings reference showing all modes (playback, navigation, queue edit, picker, library). Status bar now shows only high-priority hints; full reference lives in the modal

## 0.8.0

### Added

- **Output device selector** — press `Shift+D` to open a modal listing all available CoreAudio output devices. Current device is marked with a green bullet. Selecting a device switches playback immediately (preserving position and pause state). Choice is persisted to `[playback] output_device` in config.toml and restored on startup, with automatic fallback to system default if the device is unavailable

### Fixed

- **Stale album codec after format upgrade** — upgrading files from MP3→FLAC (or any format change) now correctly updates the album's codec in the picker. Previously `get_or_create_album()` only set codec on first insert, so the album row kept the old format even after all tracks were re-scanned
- **Streaming tracks skip mid-playback** — two issues causing premature track advancement during streaming: (1) the pump thread treated `read() → Ok(0)` as EOF even when the download reported more bytes available (OS buffer flush lag), now retries instead of breaking; (2) `refresh_track_metadata` (called when download completes mid-stream) didn't update `TrackInfo.duration_ms`, leaving the UI with an underestimated duration from the initial 256KB partial probe — now re-probes the complete file

## 0.7.2

### Fixed

- **`koan init` leaks home directory into config.toml** — `library.folders` (containing the resolved `~/Music` path) was written to the shareable `config.toml` instead of `config.local.toml`. Now `config.toml` omits `library.folders` entirely, and `config.local.toml` gets the detected music directory as a starting point

## 0.7.1

### Fixed

- **Resilient tag parsing** — files with corrupted tags (e.g. malformed UTF-16 ID3 frames) no longer fail the entire scan. When lofty errors, falls back to Symphonia for duration/codec/properties and indexes the file with whatever metadata is available
- **Suppressed library log spam** — noisy warn-level messages from lofty/symphonia internals are filtered from stderr (still written to log file). Fallback warnings from koan include the file path for diagnostics

## 0.7.0

### Added

- **DB cache for playlist loading** — when adding files that are already in the library database, metadata is pulled from SQLite instead of re-reading from disk, making re-adds near-instant
- **Scan progress bar** — `koan scan` now shows a clean inline progress indicator with track count and rate (e.g. `• 1234 scanned (567/s)`) instead of per-track log spam
- **Library source indicator** — tracks in the TUI library browser show a colored icon indicating whether they are local (green HDD) or remote (cyan cloud)

## 0.6.3

### Added

- **Sub-pixel scrollbar** — scrollbar thumb renders at 1/8th-cell resolution using Unicode block elements for smooth visual movement
- **Parallel disk scanning** — adding files from disk now uses rayon for parallel metadata reads, significantly faster for large collections

### Fixed

- **Scrollbar tracking with album headers** — scrollbar now accounts for album header lines in its position/size calculations, fixing drift when dragging and inability to scroll to the end
- **Mouse wheel scroll bounds** — wheel scrolling now correctly bounds against the display line count (including album headers) instead of just the entry count

## 0.6.2

### Added

- **WebP cover art support** — cover art in WebP format (embedded or external) is now decoded and displayed

## 0.6.1

### Added

- **Organize file path diff** — organize modal now shows a before/after visual diff of file paths, highlighting changed path segments in green
- **Ctrl-A select all** — select entire playlist from normal mode (enters edit mode) or edit mode
- **Album header context menu** — right-click on album headers to apply actions (organize, remove, favourite, etc.) to the whole album group

### Fixed

- **ALAC codec detection** — MP4 files containing ALAC audio are now correctly identified as ALAC instead of AAC, using lofty's `Mp4File` codec probe
- **Unicode string slicing panics** — fixed two panics in organize path diff caused by byte-slicing fullwidth/CJK characters; all path helpers now use char-based operations
- **Modal mode restoration** — context menu and organize modal now use a mode stack (push/pop) instead of hardcoding return to edit mode; closing a modal returns to whatever mode was active before opening it

### Tests

- **Unicode torture tests** — comprehensive coverage for fullwidth Japanese, CJK, emoji (ZWJ, flags, skin tones), Arabic bismillah, Zalgo/combining diacritics, and extreme combining mark sequences
- **ALAC codec tests** — fallback tests for `mp4_codec()` plus integration test against real ALAC files
- **Organize path diff tests** — coverage for `common_path_prefix`, `shared_prefix_len`, `truncate_path` helpers

## 0.6.0

Full codebase audit of v0.5.2 covering security, performance, architecture, dependencies, and test coverage. Every change was reviewed individually and as a combined integration.

### Fixed

- **Security hardening** — credentials removed from stored remote URLs (reconstructed from config at playback time), config and DB files restricted to 0o600 on Unix, FTS5 and LIKE query inputs sanitized, HTTPS warning for non-localhost remotes, secure random salt via `getrandom`, PID-namespaced cover art temp files
- **Streaming duration display** — seek bar metrics now use the DB-sourced track duration instead of the probed partial-file duration, so elapsed/total and click-to-seek are correct during streaming playback

### Performance

- **Render loop allocations eliminated** — playlist version gate skips redundant O(n) visible queue rebuild when queue is idle; borrowed string keys in display line builder remove 2 allocations per entry per call; spectrum data changed from heap Vec to stack arrays ([f32; 48]) eliminating allocation on every frame clone at 60fps

### Changed

- **Symphonia codec features scoped** — replaced blanket `features = ["all"]` with only the codecs koan actually uses (FLAC, MP3, AAC, Vorbis, Opus, ALAC, WavPack, WAV, AIFF), reducing compile time
- **`row_to_track_row` helper** — deduplicated 4 identical 22-line row-mapping closures in tracks.rs into a single shared function
- **`plan_single_move` helper** — extracted shared move-planning logic (path formatting, sanitization, extension preservation, ancillary file handling) from two `plan_moves` variants in organize.rs
- **rusqlite removed from koan-music** — 3 raw SQL calls replaced with koan-core query functions (`album_date`, `clear_cached_paths`). Binary crate no longer links rusqlite directly
- **rusqlite features scoped** — `bundled-full` → `bundled`, removing unused extensions (load_extension, backup, blob, hooks, session)
- **Workspace dependencies** — added `[workspace.dependencies]` for rusqlite and walkdir, centralizing version management

### Removed

- **Dead code cleanup** — removed 6 unused functions/fields: `LyricsState::clear`, `CoverArt::centered`, `scrollbar_hover` theme field, `event.rs` module, `VisualizerState::num_bars`, 3 unused `HoverZone` variants

### Tests

- **Test coverage expanded** — 332 → 371 tests. Added coverage for PlaybackTimeline (6), SharedPlayerState (12), favourites (8), Subsonic client (5), metadata probe (5). Removed 4 AI-generated duplicate streaming tests

## 0.5.2

### Fixed

- **Drag reorder selects wrong track** — dragging a single track up/down no longer switches selection to the displaced track. The dragged track's ID is now captured before the move instead of reading from the stale visible queue cache

## 0.5.1

### Added

- **ReplayGain playback support** — track and album ReplayGain tags are read via lofty at decode time and gain is applied with peak limiting. Configure via `[playback] replaygain` (`track`, `album`, or `off`) and `pre_amp_db` in config.toml. Zero overhead when disabled
- **Streaming seek bar** — during streaming playback the seek bar dims the not-yet-downloaded portion. Downloaded section renders as a solid line that grows as the download progresses. Seeking past the downloaded point is prevented (click, keyboard, and core seek all clamped)
- **Accurate duration for streaming tracks** — transport bar now prefers the database-sourced track duration over the probed partial-file duration, so elapsed/total always shows the real track length

### Fixed

- **TIFF cover art rejected** — embedded TIFF artwork is now skipped during extraction, falling back to the next JPEG/PNG picture. Fixes `CGImageDestinationFinalize failed` errors on macOS Now Playing
- **Spectrum peak markers hidden by bars** — peak hold markers now render on top of bar fill instead of being overwritten

## 0.5.0

### Added

- **Spectrum analyser** — 80s hi-fi LED-segment style spectrum visualiser renders above the transport bar when album art is present. 48-band FFT with configurable frequency scale (Bark/Mel/Log/Linear), eighth-block sub-cell resolution, green/yellow/red gradient, peak hold markers, and time-based exponential decay
- **Dedicated analysis thread** — FFT runs on a background thread (`VizAnalyzer`) decoupled from both the decode and UI threads. The UI reads a pre-computed `VizSnapshot` every frame with sub-microsecond lock hold times, ensuring buttery-smooth 60fps rendering
- **VizBuffer audio tap** — circular sample buffer shared between decode thread and analysis thread via `parking_lot::Mutex`
- **FFT pipeline** — 2048-point real FFT via `realfft` crate. Hann window, dB magnitude scaling, Bark/Mel/Log/Linear frequency scales
- **A-weighted amplitude scaling** — bars reflect perceived loudness using IEC 61672 A-weighting, matching human hearing sensitivity (Fletcher-Munson curves). Configurable via `amplitude_scale`: `aweight` (default), `perceptual` (A-weight + gamma), `sqrt`, `linear`
- **Signal-level coloring** — spectrum bars are colored by amplitude, not position. Green at safe headroom, yellow when hot, red only near clipping (0dBFS)
- **Visualiser config** — `[visualizer]` section with `enabled`, `fps` (default: 60), `scale`, `amplitude_scale`, `bar_decay_ms` (default: 50), `peak_decay_ms` (default: 180). Also accepts `[visualiser]` spelling
- **Spectrum theme colours** — `spectrum_low` (green), `spectrum_mid` (yellow), `spectrum_high` (red), `spectrum_peak` (white) in theme config
- **FPS overlay** — `[playback] show_fps = true` displays an FPS counter in the top-right corner

## 0.4.0

### Added

- **Streaming playback for remote tracks** — playback starts after 256 KB is buffered instead of waiting for the full download. A `StreamingSource` backed by a shared in-memory buffer feeds Symphonia while the download continues in the background. When the download finishes, full lofty metadata and cover art are re-read and media key info (souvlaki) is updated progressively
- **Vim-style navigation everywhere** — pickers, library browser, and queue all support Ctrl+U/Ctrl+D (half-page), PageUp/PageDown, Home/End. Library also accepts j/k/h/l, g/G
- **Wrap-around cursor** — pressing Up on the first item wraps to the last, and Down on the last wraps to the first (queue, library, picker)
- **Lyrics panel** — press `L` to toggle a lyrics panel (60/40 split with queue). Fetches synced and plain lyrics from LRCLIB (zero-config, no API key). Synced lyrics highlight the current line and auto-scroll with playback
- **Lyrics DB caching** — fetched lyrics are cached in SQLite so subsequent views are instant
- **LRCLIB search fallback** — when exact match (`/api/get`) returns 404, falls back to fuzzy search (`/api/search`) by artist + title
- **Incremental remote sync** — `koan remote sync` now only fetches albums newer than the last sync timestamp, dramatically reducing sync time. Use `--full` to force a complete re-sync
- **Resilient stale track removal** — when local files are removed, remote-backed tracks are demoted to remote-only (preserving streaming fallback) instead of being deleted entirely

### Changed

- **Fixed-timestep render loop** — replaced tick-on-timeout event loop with a game-engine-style frame-deadline loop. Animations (ticker, spinner) no longer stall during mouse interaction or key holds
- **Configurable frame rate** — `[playback] target_fps` (default: 60) controls TUI redraw rate. Accepts 30, 60, or 120
- **Transport icons** — play/pause/stop status icons use Unicode symbols instead of ASCII

### Fixed

- **Standalone picker mouse support** — `koan pick --artist`/`--album` now enables mouse capture. Click to select, double-click to confirm, scroll wheel to navigate
- **Lyrics fetch on toggle** — pressing `L` mid-track now fetches lyrics immediately. Previously, lyrics only loaded on track change
- **Lyrics error logging** — fetch errors are now logged to stderr instead of being silently swallowed
- **Favourites import for remote tracks** — starred tracks from Navidrome now correctly import as local favourites. Previously, remote-only tracks (with no local path) were silently skipped during import
- **Favourites sync error logging** — errors from `getStarred2` and `import_remote_favourites` are now surfaced instead of silently returning 0
- **Event drain starvation** — opening album art (or any slow render) no longer freezes the UI. The event loop now always polls for input even when behind on frame budget
- **Cover art zoom performance** — full-screen album art view no longer runs Lanczos3 resize every frame. Rendered output is cached and reused until terminal size changes
- **Ticker double-speed after merge** — duplicate ticker animation block from merge caused scrolling text to advance twice per frame
- **Anchored drag reorder** — dragging selected tracks now moves them anchored to the mousedown position instead of snapping to the top of the selection
- **Album header drag** — clicking and dragging an album header reorders the entire album group as a unit
- **Play/pause click** — clicking the status icon (play/pause indicator) next to the seek bar now toggles playback
- **Download progress on all tracks** — tracks before the playing position now correctly show download progress and status instead of being unconditionally marked as played

### Removed

- **Event::Tick** — tick variant removed from event enum. Ticking is now unconditional every frame

## 0.3.0

### Added

- **Ticker-style transport bar** — when the artist/title text overflows the available width, it scrolls horizontally like a ticker banner. Album, year, and codec info stay fixed. Scroll speed is configurable via `playback.ticker_fps` in config (default: 8)
- **Favourites** — press `f` to favourite/unfavourite tracks. A yellow star (★) appears in the queue gutter. Persisted to SQLite. Available in the context menu too
- **Favourite sync** — favouriting a remote track stars it on Navidrome. `koan remote sync` now pushes local favourites and pulls remote starred songs
- **Subsonic star/unstar/getStarred2 API** — new SubsonicClient methods for managing server-side favourites
- **Rich context menu** — right-click (or `Space` in edit mode) opens a positioned context menu with Play, Favourite, Track info, Remove, and Organize actions. Hotkey shortcuts work within the menu
- **Mouse hover highlighting** — queue and library items show underline on hover
- **Event drain loop** — mouse move events are coalesced so the UI always renders the latest cursor position
- **HoverZone tracking** — typed enum tracks which UI element (queue item, library item, seek bar, etc.) is under the mouse

### Changed

- **Scroll step reduced** — mouse scroll wheel moves 1 line instead of 3
- **Queue jump scroll** — `/` search now scrolls the matched track to near the top of the visible area (with album header) instead of keeping current scroll position

### Fixed

- **Scrollbar drag jump** — clicking the scrollbar thumb no longer jumps to a wrong position. The grab offset within the thumb is tracked so dragging feels natural. Clicking the track area still jumps as expected
- **Multi-select drag reorder** — dragging multiple selected tracks no longer causes chaotic oscillation. Moves only trigger when the target is outside the current selection range
- **Drag undo batching** — one drag operation (single or multi-track) is now a single undo step instead of one per row crossed

## 0.2.3

### Added

- **Cover art in Now Playing** — macOS Control Center shows embedded album art (extracted to temp file, passed as file:// URL to souvlaki)
- **Seek from Control Center** — absolute position, relative with duration, and direction-only (10s steps)
- **Quit from Control Center** — clean shutdown via atomic flag on SharedPlayerState

### Fixed

- **mise binary name** — release tarballs now contain `koan` instead of `koan-macos-arm64`, fixing mise installs

### Removed

- **Dead file watcher** — notify/FSEvents module was implemented but never wired in. Removed watcher.rs, notify deps, `config.watch` field

## 0.2.0

First public release. Full TUI rewrite, undo/redo, file organization, CI/CD pipeline.

### Added

- **Ratatui TUI** — full-screen terminal UI with transport bar (click-to-seek), album-grouped queue, fuzzy picker overlay, library browser, track info modal with embedded album art (halfblock rendering), scrollbar, mouse support throughout
- **Undo/redo** — 100-deep undo stack covering all playlist operations (add, remove, move, clear). `Ctrl+Z` to undo, `Ctrl+Y` or `Ctrl+Shift+Z` to redo. Batch operations (multi-delete, multi-move) undo as a single step
- **File organization** — in-TUI organize modal: select tracks in edit mode → `Space` → Organize → pick a named pattern → preview → execute. Playlist paths update live, playback continues uninterrupted. Ancillary files move with the music
- **Format string engine** — fb2k-compatible title formatting: `%field%` references, `[conditional]` blocks, `$function(args)` calls. 30+ built-in functions (string, logic, numeric, path). 234 tests
- **Named organize patterns** — define reusable patterns in config (`[organize.patterns]`), set a default, pick from them in the TUI modal
- **Context menu** — `Space` in edit mode opens action overlay (currently: Organize)
- **Drag/drop** — drag files or folders from Finder into the terminal to add to the queue (bracketed paste)
- **Queue editing** — edit mode (`e`) with Finder-style multi-selection (shift-arrows, option-click toggle, ctrl-click range), reorder (`j`/`k`), delete (`d`), multi-drag
- **Library browser** — split-pane tree view (artists → albums → tracks), substring filter (`f`/`/`), click/double-click support
- **Picker actions** — `Enter` appends, `Ctrl+Enter` appends and plays, `Ctrl+R` replaces entire queue
- **Mouse support** — double-click to play, click-to-seek, drag-to-reorder, scrollbar drag, scroll wheel, picker click/dismiss — works in every mode
- **Priority play** — double-click a downloading track to play it as soon as it finishes
- **Media keys** — macOS Control Center integration via souvlaki (play/pause, next/prev, now playing info)
- **Track info modal** — `i` shows full metadata + audio format details + embedded album art
- **Cover art zoom** — `z` for full-screen album art (halfblock rendering)
- **Dynamic shell completions** — `source <(COMPLETE=zsh koan)` for artist/album ID tab-completion from the DB
- **Parallel remote sync** — album detail fetches parallelized with rayon, batch DB writes per page
- **`koan init`** — scaffolds `~/.config/koan/` with config templates (organize patterns, playback defaults, library paths), database, cache dir, and `.gitignore` for dotfile repos
- **`koan pick`** — in-process fuzzy picker powered by nucleo (replaces fzf dependency). `--album`/`--artist` modes with drill-down
- **CI/CD pipeline** — test + clippy + fmt check, cross-compiled binaries (arm64 + x86_64), GitHub releases with auto-tagging, crates.io publishing (`koan-core` then `koan-music`)
- **MIT LICENSE** file

### Fixed

- **Album picker adds wrong tracks** — was passing album IDs as track IDs, now correctly expands via DB query
- **Track artist vs album artist** — stored separately in DB, compilations display correctly
- **Seek past end of track** — skips to next instead of crashing
- **Scroll past end** — queue scroll clamps correctly
- **Scroll in modals** — routes to active modal instead of always scrolling queue
- **Library shows album artists only** — no spurious entries from featured artists on compilations
- **Crash on pick subcommand** — fixed usize underflow race with `saturating_sub`, added panic hook for terminal restore
- **Queue metadata for local tracks** — was blank, now populated correctly
- **Album header dimming** — only dims when ALL tracks in group are played

### Changed

- **Crate renamed** — `koan-cli` → `koan-music` (binary stays `koan`), directory `crates/koan-music/`
- **Config path** — `~/.config/koan/` (was `~/Library/Application Support/koan/`)
- **Two-layer config** — `config.toml` (committable) + `config.local.toml` (gitignored)
- **Password storage** — stored in `config.local.toml` via `koan remote login`, not macOS Keychain

### Removed

- **`koan organize` CLI subcommand** — file organization is now TUI-only (context menu → organize modal)
- **FFI/Swift layer** — removed entirely, pure Rust
- **fzf dependency** — replaced with built-in nucleo fuzzy picker

## 0.1.0

Initial release.

- Bit-perfect CoreAudio playback (AUHAL, automatic sample rate switching)
- Gapless transitions
- Format support: FLAC, MP3, AAC, Vorbis, Opus, ALAC, WavPack, WAV/AIFF (Symphonia)
- Library indexing with rayon, SQLite FTS5 search
- Subsonic/Navidrome remote sync
- Track deduplication (path → remote_id → content match)
- CLI: play, scan, search, library, config, probe, devices, remote login/sync/status
