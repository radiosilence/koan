# Link

A kōan app signed in to a kōan server holds a WebSocket open to it, so the server can command the app and the account's devices can command each other through it. Subsonic has no way for a server to reach a client. The same messages also run over [nearby](nearby-protocol.md) connections between devices on one network.

The Rust types are `LinkCommand` and `LinkReport` in `koan_core::remote::link`, `Envelope` and `AckOutcome` in `koan_core::remote::acks`.

## Connecting

`GET /rest/koanLink` upgrades to a WebSocket, authenticated like any `/rest` call (`koanLink` extension). Parameters:

| Parameter | Meaning |
|-----------|---------|
| `client` | The device's display name |
| `platform` | `macos`, `ios`, `tvos`, `android`, `linux` |
| `device` | A stable id for the device. Without one, each connection is a device of its own |
| `devices=1` | Send this link the account's devices and shares ([Devices](#devices)) |
| `acks=1` | This device answers commands sent with an id ([Acks](#acks)) |
| `deviceKey` | Base64 of the device's 32-byte Ed25519 public key (`koanDeviceKeys`). Kept only when the link signs in with an API key, on that key's row; passing it at all asks for `deviceKeys` |

A new link from the same `device` replaces the old one. The server closes a link when its account changes in a way that could narrow what it may do: its role, its password, its deletion, or a key or app password revoked. The client reconnects, as after any close, and is authenticated afresh. The server drops a link it has heard nothing from for 100 seconds, so a client pings while idle.

## Messages

Each WebSocket text frame is one JSON object tagged by `type`, with camelCase fields (one exception: `unix_ms` in a `state`'s `sleep`). Down the link the server sends commands and news (`LinkCommand`); up it the client sends reports (`LinkReport`). Track ids are the server's track uids.

A field marked `?` may be absent, and senders leave most of them out when they hold their default: `false`, `0`, empty, or none. Every other field is required.

### Commands

| `type` | Fields | Effect |
|--------|--------|--------|
| `play` | `trackIds`, `startAt?`, `positionMs?`, `paused?`, `handoff?`, `mode?` | Replace the queue with these tracks and play from `startAt`, `positionMs` into it, or load paused. `mode` `inOrder` or `shuffled` resets the play mode, as a play button does; `keep`, or absent, keeps it. `handoff` marks music handed over from another device: a receiver controlling another device takes control back |
| `enqueue` | `trackIds`, `play?` | Append; with `play`, start the first of them |
| `playNext` | `trackIds` | Insert after the current track |
| `insert` | `trackIds`, `after` | Insert after the queue entry `after` |
| `remove` | `trackIds` | Remove every entry for these tracks |
| `removeItems` | `ids` | Remove these queue entries |
| `moveItems` | `ids`, `target`, `after` | Move entries before or after `target`, in the order given |
| `playItem` | `id` | Play this queue entry. Unlike `jumpTo` it names one entry of a track queued twice, and reaches a file only that device has |
| `jumpTo` | `trackId` | Play this track, from the queue or slotted in after the current one |
| `clear`, `pause`, `resume`, `next`, `previous`, `undo`, `redo` | | As named |
| `seek` | `positionMs` | |
| `shuffle` | `on` | The queue keeps its order; shuffle picks which unplayed track plays next |
| `repeat` | `mode`: `off`, `queue`, `one` | |
| `sleepTimer` | `timer?`: `{"kind":"after","minutes":n}`, `{"kind":"endOfTrack"}`, `{"kind":"endOfRecord"}`; `null` or absent cancels | |
| `sync` | `full?` | Pull what the server changed: library, favourites, playlists. The library is walked only if the server says it moved, unless `full` |
| `evict` | `trackIds` | Delete downloaded copies, so the next play fetches them again |
| `handOff` | `to` | Send this device's queue and playhead to device `to` as a `play` with `handoff`, and pause here |
| `setOutput` | `output`: `{"kind":"default"}`, `{"kind":"device","name":…}`, `{"kind":"renderer","udn":…}` | Play through this output from now on |
| `refreshOutputs` | | List outputs again, and report them if they moved |
| `setRendererVolume` | `volume` (0–100) | The volume of the UPnP renderer the device plays to |
| `setPreset` | `device`, `profile?` | Play output `device` through DSP profile `profile`, or untouched with `null` |
| `watchLevels` | `on` | Report audio levels while `on`: a controller has playing bars on screen |

### News from the server

| `type` | Fields | Meaning |
|--------|--------|---------|
| `devices` | `devices` | The account's devices, as they are now ([Devices](#devices)) |
| `deviceKeys` | `keys`: `[{"id","key","owner?"}]` | The public keys of the account's devices and those shared with it (`koanDeviceKeys`); `owner` set for the latter |
| `shares` | `grantees`, `accounts?`, `error?` | The accounts this device lets control it, every account to choose from, and why the last change was refused ([Shares](#shares)) |
| `shared` | `command` | A command from another account this device is shared with |
| `forgotten` | `device` | That device was removed from the account |
| `historyChanged` | | The account's play history moved; read `koanHistory` |
| `dspProfilesChanged` | | The account's EQ profiles moved; read `koanDspProfiles` |
| `levels` | `from`, `f` | A frame of audio levels from a device this one watches |
| `acked` | `from`, `ack`, `outcome` | A device answered a command this one sent ([Acks](#acks)) |

### Reports

| `type` | Fields | Meaning |
|--------|--------|---------|
| `state` | see below | What the device is playing |
| `push` | `token`, `sandbox` | Where Apple's push service reaches this device once iOS suspends it; `sandbox` for a development build |
| `command` | `to`, `command`, `ack?` | Send `command` to device `to` ([Devices](#devices)) |
| `received` | `ack` | A command sent under `ack` came off this link, before it is acted on |
| `ack` | `ack`, `outcome` | This device's answer to a command sent under `ack` |
| `activity` | `token?`, `device?`, `sandbox?` | Where to push updates to a Live Activity showing device `device`; `null` for both when the activity has ended |
| `wake` | `to`, `notify?` | Wake device `to`, which is not linked: with a background push, or with `notify` a notification to tap |
| `share` | `grantee`, `allow` | Let account `grantee` control this device, or stop letting it. Only ever about the device sending it |
| `forget` | `device` | Remove a device from the account |
| `levels` | `f` | A frame of this device's audio levels, while watched |

`LinkReport` has one more variant, `hello`, which only a [nearby](nearby-protocol.md#handshake) listener sends.

`state` requires `playing`, and may carry `title`, `artist`, `album`, `positionMs` (as of sending), `durationMs`, `queue`, `outputs`, `shuffle`, `repeat`, `sleep` and `sleepFading`. `queue` is the queue, or the part around the current track when it is long; each entry requires `title`, `artist` and `current`, and may carry `id` (its queue entry id on that device), `trackId` (absent for a file only that device has), `album` and `durationMs`. A device sends `state` when something changes beyond the playhead moving at one second per second: a seek, a pause, another track, an edited queue.

`sleep` is the sleep timer as set: `{"kind":"at","unix_ms":n}` (when it ends, ms since the epoch), `{"kind":"endOfTrack"}` or `{"kind":"endOfRecord"}`.

`outputs` is what the device can play through: `devices` (its own audio outputs) and `renderers` (the UPnP renderers it sees), each a list of `{"id","name","kind?","detail?","busy?","preset?","unsaved?"}`; `current`, an output as `setOutput` takes it; `volume?`, the renderer's volume; `profiles`, its EQ presets; and `dspEnabled`. `kind` is how an output is connected (`usb`, `bluetooth`, `upnp` and others), `detail` a renderer's make and model, `busy` that it is playing for something else, `preset` the EQ preset it was set from, and `unsaved` that its EQ has changed since.

A levels frame `f` is an array `[atMs, low, mid, high]`: milliseconds into the track it was heard at, then three bands in thousandths (0–1000).

## Devices

`koanDevices`. A link opened with `devices=1` is sent `devices` when it links and whenever one of the account's devices changes. Each device has `id`, `name`, `platform`, `linked`, and may carry `state` (its last report, with the playhead placed as of sending), `lastSeen` (Unix seconds, for one not linked), `wakeable` (the server can push to it), `owner` (for a device another account shares) and `acks`.

A device commands another with a `command` report. The `ack`, when there is one, sits beside `to` and `command`, not inside `command`:

```json
{"type":"command","to":"0192…","command":{"type":"pause"},"ack":7}
```

`to` names the first linked device whose id, link id or name (without regard to case) equals it; a client should send the device id, since names need not be unique and one could equal another device's id. The server relays the command when `to` is one of the caller's devices or one shared with the caller. Only commands one device gives another are relayed (`LinkCommand::relayable`): the server's own news (`devices`, `deviceKeys`, `shares`, `shared`, `forgotten`, `historyChanged`, `dspProfilesChanged`, `levels`, `acked`) is refused, so a forged copy cannot pass as the server's. Levels go only between live links, by their own route: a controller sends `{"type":"command","to":…,"command":{"type":"watchLevels","on":true}}`, and the server forwards the watched device's `levels` reports to it as `levels` news. `watchLevels` works only as a link report; `koanCommand` refuses it.

A command for a device that is not linked is kept and delivered when it links. An iOS device that gave a push token is woken, if the server has a `[push]` key. `watchLevels` and `refreshOutputs` are only worth saying to a device that is there, so they are never kept or pushed.

`/rest/koanCommand?to=<device id>&command=<command JSON>` sends one command in a single request, for a device whose own link is down, such as an iPhone acting from its lock screen. `ack` optionally names the command's id.

## Acks

`koanAck`. A command that wants an answer carries `"ack": <u64>` in the same JSON object as the command (an `Envelope`), never inside the `LinkCommand` itself: a server that predates acks re-serialises relayed commands and would drop it, while a client that predates them ignores it.

```json
{"type":"pause","ack":7}
```

The server attaches an id only for a device that linked with `acks=1`. That device reports `received` as soon as the command comes off its link, then `ack` once it has acted, with `outcome` one of `{"result":"done"}`, `{"result":"refused","reason":…}` or `{"result":"failed","error":…}`. The server relays the answer to the sender as `acked`. It also answers on the device's behalf, with `from` naming the device: `queued` for a device that is away, `refused` when it will not relay the command (not the caller's device, or not relayable), and `failed` when the device's link went as the command was sent. A link that has not shown a command as `received` within a moment is taken as gone: the server drops it, wakes the device, keeps the command for it and answers `queued`.

Ids are random unsigned 64-bit integers across the whole range, so a client must read and echo them as integers without rounding them to a double. A sender that hears nothing tries the next route with the same id, so a command can arrive twice; a device remembers the ids it has taken for a minute, and answers a repeat with the first outcome without acting on it again.

`koanCommand` waits up to four seconds for the answer and returns it as a `koanCommand` element with `result`, and `reason` or `error`. With no answer in that time, the response is empty: sent. A command the server will not deliver is not answered with `refused` or `failed` here but with a Subsonic error: 10 for a command it cannot parse, and 70 when `to` matches no device of the caller's or shared with it, when the command is not relayable, or when the device's link dropped as it was sent.

## Shares

`koanShares`. A device reports `share` about itself only. A link opened with `devices=1` is sent `shares` when it links and whenever the list changes. A shared device appears in each grantee's `devices` with its `owner`, and a grantee's command reaches it wrapped as `shared`. It runs as the grantee's request, from the playback set: transport, the queue, the sleep timer, the volume, the output and preset, and moving the music (`LinkCommand::allowed_playback`; the list is on [Nearby](nearby-protocol.md#what-a-peer-may-do)). Nothing a shared command does changes the owner's library, favourites, playlists or history, and a track it names that the library lacks is not synced for.

The other way, a shared device may send its grantee's devices one command: `play` with `handoff`, which is how Move here hands the music to a grantee. The server delivers it wrapped as `shared` and refuses anything else.
