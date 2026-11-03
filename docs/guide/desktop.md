# The desktop app

kōan's desktop app runs on macOS and Linux. It is drawn with GPUI and links the same engine the native apps do, so it plays bit-perfect through CoreAudio on a Mac and through ALSA, PipeWire or PulseAudio on Linux. Windows is waiting on the engine building there ([#1600](https://github.com/radiosilence/koan/issues/1600)).

## Installing

Each release carries a DMG for macOS 26 or later on Apple silicon, and an AppImage and a `.deb` for Linux on x86-64 and ARM64, which need glibc 2.35 or later (Ubuntu 22.04, Debian 12 and their successors). On a Mac it is `Koan.dmg`, and installs as `kōan desktop.app`.

## Beside the other apps

The desktop app is this computer to the server and the network: named after it (`studio`), with the computer's device id, saved session and the devices it knows. The terminal UI on the same library is a device of its own, `studio (terminal)`, with a session of its own, so one can control the other. Only one desktop app runs per configuration directory: opening it again, or opening files with it, brings the running one to the front and hands it the files.

## Taking over from the Mac app

The SwiftUI Mac app is no longer released from 0.70. The desktop app's first launch on a configuration the Mac app used takes the Mac app's place: the phones, the TV and the server go on seeing one Mac, with its play history, its TV pairings and its saved queue. If only the desktop app ever ran there, it keeps the device it already was, and its own queue.

The desktop app does not open while the Mac app is running, since the two would be the same device: quit the Mac app first. A Mac app from 0.70 still installed and opened on that configuration afterwards is a device of its own named `studio (legacy)`, with a session of its own, so it cannot take the desktop app's place. An older Mac app knows nothing of this and, opened while the desktop app runs, is the same device as it, so remove it once the desktop app has taken over.

## Files from outside

Files and folders dropped on the queue are added to the library where they lie and appended to the queue; dropped on a playlist, they are added to it. Folders are walked, so dropping a rip queues the album. On Linux, files opened with the app from the file manager, and on both, files named on the command line (`koan-gui ~/Music/new/*.flac`), are queued the same way. Nothing is moved: [organize](file-organization.md) is what puts them under a library folder.

The same context menu has download to cache, which fetches a remote library's tracks onto this device now, and remove downloaded files, which asks first and deletes those copies; they stay on the server and download again when played. The queue's menu has them too, with organise.

Organise… in the context menu of a track, a record, an artist or a pick opens a sheet over the window: a pattern from `[organize.patterns]` or one typed for this once, the library folder to move into, and whether cover art, cue sheets and logs go with the music. A pattern typed or changed can be saved under a name, which stores it in `config.toml` for every kōan on that configuration; the sheet says whether the name is new or replaces one before it saves. Every file is listed with where it would go, as the pattern is typed; once typing rests the plan is checked against the disk, so a destination already taken shows as blocked before anything moves. Organising asks once more with the counts. Nothing is ever overwritten.

Invite links and `koan.rocks/pair/` links opened with the app go to Settings: an invite asks first whether to sign in as its account on its server, saying what becomes of the account signed in now, and changes nothing until you say so (one for the account already signed in just says so); and a pairing link asks whether to sign that device in to this account. A pairing link for another server is refused with the reason.

## Media keys and now playing

The app shows what is playing in the system's controls, cover included, and takes their commands: Now Playing and the media keys on a Mac, MPRIS on Linux, so `playerctl` and the desktop's own media controls work. While it controls another device, the media keys control that device.

## The Dock

On macOS the desktop app's Dock menu shows what is playing, with its heart, and has repeat, shuffle, play and pause, next and previous, the sleep timer and Play On, which lists the devices you can control from here. Its icon has a spindle at the circle's centre, so it is told apart from the old SwiftUI Mac app in the Dock and the app switcher.

## Keys

The same shortcuts on both, with `Ctrl` for `⌘` on Linux. Linux has no global menu bar, so the menus a Mac shows along the top of the screen open from the ⋯ button at the left of the toolbar instead, each item with its keys. See [Keybindings](../reference/keybindings.md#desktop-app), or press `?` in the app.

## Configuration and the log

If the library cannot be opened (a database a newer kōan has upgraded, another desktop app already running on the same configuration, or, on a Mac, the Mac app running as this device), the app says so in a window, with what to do about it, in the words every front end uses, and quits when it is closed.

The app reads the configuration every kōan front end does, `~/.config/koan/` (or `KOAN_CONFIG_DIR`), and writes its log to `koan.log` there, and to stderr when started from a terminal. `KOAN_LOG=debug` adds kōan's debug lines, such as how many covers are being fetched at once.

Settings → Devices → In the background keeps kōan running with its window closed, so your other devices can still see and control this computer: on macOS it moves from the Dock to the menu bar, whose item shows what is playing and opens the window again; on Linux, opening kōan again brings the window back. Open at login asks the system to start kōan when you log in (Login Items on macOS, which may ask for approval in System Settings; an autostart entry on Linux). A computer cannot be woken from another device, so a kōan that has quit is out of reach until it is opened again.

What the app writes as it runs stays out of the configuration directory, which may be kept in a dotfiles repository: the window's place, the browsers' sorts, the desktop device's id and the devices it knows go to the platform's state directory (`~/Library/Application Support/koan/` on macOS, `$XDG_STATE_HOME/koan/` on Linux), its locks to `$XDG_RUNTIME_DIR/koan/` on Linux, and the artwork and Now Playing's covers to the platform's cache. Each sits under a folder named for the configuration directory (`default` for your own), so a throwaway `KOAN_CONFIG_DIR` keeps them apart. `KOAN_PLACES_DIR` puts all of them in one directory instead (`state/`, `runtime/` and `cache/` inside it), which `just gui-run` and the examples point inside their throwaway configuration, so deleting it deletes them. Files an earlier version left in the configuration directory are moved there on the first launch.

## Running a build from source

```bash
dir=$(mktemp -d)
KOAN_CONFIG_DIR=$dir KOAN_PLACES_DIR=$dir/places cargo run --release -p koan-gui --features runtime-shaders
```

On a Mac, `runtime-shaders` has GPUI compile its Metal shaders when the app starts, so the Command Line Tools are enough to build it (`just gui-run` turns it on). Without it the shaders are compiled with the app, which needs Xcode's Metal toolchain; releases are built that way.

A build from `main` can move the library database to a newer schema than the released apps read, after which they refuse it; a throwaway `KOAN_CONFIG_DIR`, with `folders` in its `config.toml` pointed at some music, keeps your own library out of it.

## The window

While this device cannot use its server (signed out, its sign-in refused, or the server out of reach), a line across the top of every page says so, with sign in where signing in would help. Offline by choice shows nothing.

The sidebar lists the queue, the library's sections and your playlists. Back and forward move through the pages visited, as a browser does, from the toolbar, with ⌘[ and ⌘] (Ctrl on Linux), or with a mouse's back and forward buttons. The transport runs along the foot: the record playing, the controls over the seek slider, the format, the devices, sleep timer and output menus, and EQ, in the accent while one is in force. The output menu lists this computer's audio devices and the UPnP renderers on the network, which it searches for each time it opens, so a renderer switched on since appears within a couple of seconds; the renderer in use shows its volume. Each browser narrows its listing from the filter field in the toolbar. The window opens where it was left.

The window follows the system's display settings as they change. With Increase Contrast on a Mac, or high contrast on GNOME, text and rules are drawn stronger, the record's colour is held to a contrast of 7:1, and picked rows are outlined. Reduce Transparency on a Mac takes away the colour wash behind the pages. Reduce Motion stops every animation. On GNOME the system's text scaling sets the starting text size, and Settings → Appearance → Text size scales it from there.

## The queue

The queue is grouped by record: a heading with the sleeve over each record's run, and a track standing alone with its own sleeve. The playing track is in the accent colour.

| Action | How |
|--------|-----|
| Play a track | Double-click it, or select it and press Return |
| Play or pause | Space, anywhere in the window |
| Select | Click; ⌘-click adds or removes a row, Shift-click selects a range. A heading selects its record |
| Move the selection | Up and Down; with Shift, extend it |
| Select everything | ⌘A |
| Remove | Delete or Backspace, or the context menu |
| Reorder | Drag rows; dragging a heading moves its record, and a selected row moves the whole selection |
| Undo, redo | ⌘Z, ⇧⌘Z |
| Follow what's playing | The crosshair in the toolbar keeps the playing track in view as the queue plays and changes. It stops when you scroll the queue, jump to its top or end, press the crosshair again, or nothing is left under the cursor |

The queue keeps its place and selection when you visit another page and come back.

A row's context menu acts on the rows under it, or the selection: play, remove, add to playlist, organise, the files on this device, go to album and a share link. A heading's acts on its whole record, and its share link names the record. A share link is made on your server and copied; only tracks the server holds go in it, and the notice says how many were left out.

Playlists in the sidebar are reordered by dragging them; dragged onto the queue's row, or into the queue, a playlist's tracks are queued. "new" beside the playlists heading makes an empty playlist, and tracks, records or artists dropped on the heading make one holding them; "add to playlist" in any context menu offers a new playlist too, and the queue's ⋯ saves the queue as one. A playlist's context menu in the sidebar plays or shuffles it, and renames, exports or deletes it, asking first.

A playlist's page is laid out as the queue is, grouped by record or flat (the album toggle in the toolbar, remembered per playlist). Play and shuffle are in its header; the toolbar holds undo and redo, and under ⋯ play next, add to queue, shuffle the order, rename, export as M3U8 and delete. Rows are selected as in the queue and dragged to reorder, with the insertion line showing where they land; tracks, records and playlists dropped between rows are added there, and files dropped on the page are added at the end. Delete removes the selected rows. Removing, shuffling the order, renaming and deleting say what will change before they do it, and every edit but deleting can be undone. Smart playlists, playlists read from files in the library and playlists the server marks read-only say so in the header and take no edits.

## Favourites, Recently played and Downloaded

Each of these shows the first few artists, records and tracks on it, artists as names, records as tiles and tracks as rows, with how many there are of each in all. A section's heading opens the whole listing of that kind on the shelf (favourite albums, artists played in the last month, tracks on this device) on a page of its own, which starts with what the preview showed and leaves the library's own browsers filtered as they were. Clicking a name opens the artist; ⌘-click (Ctrl on Linux) starts picking, after which clicks tick artists, records and tracks alike, Shift-click ticks a range and ⌘A ticks everything on the page. Dragging anything picked carries the whole pick to the queue or a playlist, and the context menu acts on it.

## History

History lists every play, most recent first, under a heading per day, with the time each started where a record's page puts the track number. A track played three times is three rows. Plays another client scrobbled to the server are marked. Rows play, queue and drag as the library's do; Delete forgets the selected plays and "remove from history" in the context menu forgets those under the pointer, and the toolbar's ⋯ clears everything. Each asks first, naming what goes; signed in to a kōan server, forgetting reaches every device on the account. The toolbar's filter narrows it as it does the browsers.

## Search

⌘K, `/`, ⇧⌘K (File → Find and Play…, Ctrl on Linux) or a click in the sidebar's search field opens the search box over the window. It searches as you type and lists up to five tracks, five records and five artists, then "All results…"; in a small window the list scrolls between the query and the keys. Everything in it is done from the keyboard:

| Key | Does |
|---|---|
| Return | With nothing lit, or "All results…", every result on a page of its own, whose headings open each kind's whole listing; on a result, go to it: a record or an artist to its page, a track to its record's page with the track selected |
| ↑ ↓ | Light a result, or "All results…" after the last |
| ⌘Return | Play next |
| ⌥Return | Add to the queue |
| ⇧⌘Return | Replace the queue, asking first when it holds anything |
| Tab, ⇧Tab | Tick or untick the lit result; ⌘Return, ⌥Return and ⇧⌘Return then act on everything ticked |
| ⇧Return | Every result, from anywhere in the list |
| Escape | Clear the ticks, then empty the query, then close the box |

Choosing a result never plays it: only ⌘Return, ⌥Return and ⇧⌘Return do. A click on a result does what Return does, and ⌘-click ticks it.

## Lyrics, track info and artwork

The toolbar's lyrics button, ⌥⌘L or `L` shows the playing track's lyrics beside the page. Synced lyrics follow the song with the line being sung larger and kept in view, and a click on a line seeks to it; lyrics without timings are shown whole. They come from the file, a sidecar or LRCLIB, which the panel's head names.

A track's context menu has Track Info, which lists what each of its sources says (the file's tags, the server's entry), its ReplayGain and the library's ids for it. A click on a record page's sleeve, or on the transport's cover, shows the cover as large as the window allows; a click anywhere or Escape closes it.

## Settings

⌘, (Ctrl+, on Linux), or kōan → Settings…, opens Settings in a window of its own; `koan-gui --settings` opens it at launch. Its tabs are Library, Server, Account, People, Playback, EQ, Devices, Integrations and Appearance, the middle ones where a server you are signed in to offers them. The EQ tab says what the output in use plays and opens the EQ window, below, where profiles are made and chosen.

Settings reads the configuration again when its window opens and whenever it comes to the front, so a change made in the terminal UI or the CLI shows rather than being overwritten. A change writes only the setting it changes. Anything that removes or replaces something (a folder's tracks, the library index, downloads, a key, an account, sharing) says what it will do and asks first. The line along the foot says what is under way, and then how it went.

Downloads, in the sidebar, lists what is being fetched from your server and what finished or failed, with the cache's size against its limit. A running download shows its rate and a bar; one that has stopped moving says it has stalled, and a failure keeps its reason. Retry fetches a failed track again; cancel stops one, and queue entries waiting on it are skipped until it is retried; remove deletes a downloaded copy, which downloads again when played. Clear finished forgets the settled rows and leaves the files; a cancelled download stays listed, since its row is how it is retried. While any are running, the sidebar's Downloads row counts them. Wherever a track's row says where its file is (the queue, the library, playlists), a download in progress fills a ring as it arrives.

## EQ

The transport's EQ button chooses the output's preset, or no EQ, and Edit… opens the EQ window on it. The glyph is plain while the output plays untouched and takes the accent while an EQ is in force, whose name its tooltip gives. Playback → EQ… (⌥⌘E, Ctrl+Alt+E on Linux) opens EQ in a window of its own, on the output in use; the device menu at its top picks another. It is the same EQ the other apps have, described in [Equalisation and convolution](dsp.md): the chain a device plays, its preset, the curve of the whole, and each profile's page with its bands. Drag a band's handle on the graph for its frequency and gain, with Option held (Alt on Linux) for its Q; a graphic curve is painted by dragging across the graph away from the handles. Files dropped on the window are imported into the device's filters.
