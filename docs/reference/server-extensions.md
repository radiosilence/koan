# Server extensions

kōan's server speaks Subsonic, and adds what Subsonic has no way to say: a server reaching a client, devices commanding each other, signing in without typing a password into every device, and per-account state such as play history and EQ that every device shares. Each addition is listed as an OpenSubsonic extension in `getOpenSubsonicExtensions`, and a client decides what to offer from that list rather than from the server calling itself kōan. A kōan server older than an extension does not list it, and a server that is not kōan may list one.

This page is the specification of each extension: what it is for, its endpoints and messages, and who may call them. kōan's own clients implement the client side in `koan_core::remote`; any other client can use the same endpoints.

## Conventions

Unless an endpoint says otherwise:

- Every endpoint is under `/rest/` and authenticates like any other Subsonic call: `u` with `p` or `t`+`s`, or `apiKey`. `f=json` returns JSON, otherwise XML, inside the usual `subsonic-response` envelope.
- Parameters may be sent in the query string or, under OpenSubsonic's `formPost`, in an `application/x-www-form-urlencoded` POST body. Endpoints marked **POST** accept only POST; secrets they take belong in the body, where proxies do not log them.
- Times are milliseconds since the Unix epoch, except where an attribute is an ISO 8601 string.
- Track ids are the server's track uids, the ids `getSong` and the rest publish.
- An answer with no element named is an empty `ok` response.

Credentials differ in what they may do. An account's **own password** and its **API keys** may manage the account's credentials and its EQ. An **app password** is a credential handed to one third-party app, and the server's **shared secret** (`[subsonic]`) belongs to no account; neither may make keys, approve pairings or reach EQ. See [Authentication](../guide/authentication.md).

## Sign-in and accounts

### `koanSignIn`

Trades the account's password for an API key, so a device sends the password once and keeps a key of its own.

`GET|POST /rest/koanSignIn?name=<device>`, signed with the account's own password (`p=enc:…` works over plain HTTP). Answers `join` with `username` and `apiKey`. The key replaces any of the account's keys with the same `name`, so a device that signs in again holds one key. Any other credential is refused with error 50.

### `koanInvite`

Invite links: an admin mints a signed token for an account, and a device trades it for an API key without the password.

| Endpoint | Caller | Parameters | Answer |
|----------|--------|------------|--------|
| `koanJoin` | none: the token is the credential | `invite`, `name` | `join` with `username`, `apiKey` |
| `koanInvite` | admin | `username`, `reset=true` to set a new random password and disconnect the account's links | `invite` with `username`, `token`, and `password` when reset |
| `koanCreateUser` | admin | `username`, `role` (`admin`, `user`, `readonly`) | `invite` with `username`, `token`, `password` |
| `koanUsers` | admin | | `users`, one `user` per account with `username`, `role` |
| `koanSetUserRole` | admin | `username`, `role` | empty |
| `koanDeleteUser` | admin | `username`; not the caller's own | empty; the account's links are disconnected |
| `koanRevokeKey` | API key | | empty; revokes the key the request is signed with |

The admin endpoints need an admin's own password or API key. `koanRevokeKey` is how an app gives up a key it has replaced, such as the one a join superseded.

### `koanPasswords`

`POST /rest/koanSetUserPassword` with `password`, and `username` (default: the caller). An admin may set any account's password; anyone else only their own, and must give the current one as `current`. A wrong `current` counts against the account's sign-in limit and answers error 0, not 40, so an app does not read it as its own sign-in failing.

Every session, key, app password and link of the account ends. Changing one's own password answers `join` with a new API key named `name`, since the key the request came with was among those revoked; setting another account's answers empty. Signed with an API key or the account's own password.

### `koanApiKeys`

The account's own API keys. Signed with an API key or the account's own password.

| Endpoint | Parameters | Answer |
|----------|------------|--------|
| `koanApiKeys` | | `apiKeys`, one `apiKey` per key with `id`, `name`, `created`, `lastUsed` (ISO 8601), and `current` for the key signing this request. Never the key itself |
| `koanCreateApiKey` (**POST**) | `name` | `apiKey` with `id`, `name`, `key`. The key is shown once |
| `koanRevokeApiKey` (**POST**) | `id` | empty. Refuses the key signing the request: that is signing out |

### `koanAppPasswords`

The account's app passwords, for Subsonic apps that sign in only with a `t`/`s` token. Listed only when the server has a signing key, which app passwords are sealed under. Signed with an API key or the account's own password.

| Endpoint | Parameters | Answer |
|----------|------------|--------|
| `koanAppPasswords` | | `appPasswords`, one `appPassword` per password with `id`, `name`, `created`, `lastUsed`. Never the password |
| `koanCreateAppPassword` (**POST**) | `name` | `appPassword` with `id`, `name`, `password`. Shown once; error 0 when the account holds as many as it may |
| `koanRevokeAppPassword` (**POST**) | `id` | empty |

### `koanPair`

Signing in a device with no keyboard, such as a television.

1. The device opens a WebSocket at `/rest/koanPair?name=<device name>` with no credentials. A request carrying an `Origin` header is refused, so a web page cannot open one. The server sends `{"type":"pending","id":…,"code":"XXXX-XXXX","expiresIn":600}`. The device shows the code and a `koan.rocks/pair/<id>` link.
2. Someone signed in looks it up with `koanPairInfo?pair=<id or code>`, which answers `pair` with `device` (its name), `from` (its address) and `local` (whether that address is on a private network), and asks the person whether to sign it in.
3. `koanPairApprove?pair=<id or code>` mints an API key on the approver's account and sends `{"type":"approved","username":…,"apiKey":…}` down the waiting socket. With `decline=true` it sends `{"type":"declined"}`. Approving needs an account's own password or API key; declining does not.

A pairing lives in memory for ten minutes or until its socket closes, after which the device is sent `{"type":"expired"}`. Opening one is rate-limited per address, with at most three waiting per address. The web UI's `/pair` page approves through the same path.

### `koanMcp`

`/rest/koanMcp` answers `mcp` with `url` (the server's `/mcp` endpoint) and `connect` (the page explaining how to connect an assistant). Listed only when `sharing.public_url` is set, since MCP's OAuth sign-in needs the address the server is reached at. See [MCP integration](../guide/mcp-integration.md).

## Devices

### `koanLink`

A kōan client's standing connection to its server, so the server can command it. Subsonic has no way for a server to reach a client.

`GET /rest/koanLink` upgrades to a WebSocket, authenticated like any `/rest` call. Parameters:

| Parameter | Meaning |
|-----------|---------|
| `client` | The device's display name |
| `platform` | `macos`, `ios`, `tvos`, `android`, `linux` |
| `device` | A stable id for the device. Without one each connection is its own device |
| `devices=1` | Send this link the account's other devices (`koanDevices`) |
| `acks=1` | This device answers commands sent with an id (`koanAck`) |
| `deviceKey` | Base64 of the device's Ed25519 public key (`koanDeviceKeys`) |

Both directions carry JSON text frames tagged by `type`, in camelCase. Down the link the server sends `LinkCommand`s; up it the client sends `LinkReport`s. A newer link from the same `device` replaces the old one. The server closes a link when the account changes in a way that could narrow what it may do (role, password, deletion, a revoked key), and the client reconnects and is authenticated afresh. A link silent for 100 seconds is dropped, so clients ping while idle.

Commands the server sends:

| `type` | Fields | Effect |
|--------|--------|--------|
| `play` | `trackIds`, `startAt`, `positionMs`, `paused`, `handoff`, `mode` (`inOrder`, `shuffled`; absent keeps the modes) | Replace the queue and play |
| `enqueue` | `trackIds`, `play` | Append, and with `play` start the first |
| `playNext` | `trackIds` | Insert after the current track |
| `insert` | `trackIds`, `after` (a queue entry id) | Insert after that entry |
| `remove` | `trackIds` | Remove every entry for these tracks |
| `removeItems` | `ids` | Remove these queue entries |
| `moveItems` | `ids`, `target`, `after` | Move entries before or after `target` |
| `playItem` | `id` | Play this queue entry |
| `jumpTo` | `trackId` | Play this track, from the queue or slotted in after the current one |
| `clear`, `pause`, `resume`, `next`, `previous`, `undo`, `redo` | | As named |
| `seek` | `positionMs` | |
| `shuffle` | `on` | |
| `repeat` | `mode` (`off`, `queue`, `one`) | |
| `sleepTimer` | `timer`: `{"kind":"after","minutes":n}`, `{"kind":"endOfTrack"}`, `{"kind":"endOfRecord"}`, or `null` to cancel | |
| `sync` | `full` | Pull the library, favourites and playlists; `full` walks the library even if the server says it has not moved |
| `evict` | `trackIds` | Delete downloaded copies, so the next play fetches them again |
| `handOff` | `to` | Send this device's queue and playhead to device `to` as a `play` with `handoff`, and pause here |
| `setOutput` | `output`: `{"kind":"default"}`, `{"kind":"device","name":…}` or `{"kind":"renderer","udn":…}` | Play through this output |
| `refreshOutputs` | | List outputs again and report them if they moved |
| `setRendererVolume` | `volume` (0–100) | |
| `setPreset` | `device`, `profile` (or `null`) | Play an output through a DSP profile, or untouched |
| `watchLevels` | `on` | Report audio levels while a controller shows them |
| `historyChanged` | | The account's history moved; read `koanHistory` |
| `dspProfilesChanged` | | The account's EQ profiles moved; read `koanDspProfiles` |

Reports the client sends:

| `type` | Fields | Meaning |
|--------|--------|---------|
| `state` | `playing`, `title`, `artist`, `album`, `positionMs`, `durationMs`, `queue`, `outputs`, `shuffle`, `repeat`, `sleep`, `sleepFading` | What the device is playing. `queue` is the queue, or the part around the current track when it is long; each entry has `id`, `trackId` (absent for a file only the device has), `title`, `artist`, `album`, `durationMs`, `current`. Sent when something changes beyond the playhead moving at one second per second |
| `push` | `token`, `sandbox` | Where Apple's push service reaches this device once iOS suspends it |

Further commands and reports belong to the extensions below.

### `koanDevices`

A person's devices seeing and commanding each other through the server. Links opened with `devices=1` are sent `{"type":"devices","devices":[…]}` whenever one of the account's devices changes. Each device has `id`, `name`, `platform`, `linked`, `state` (its last `state` report, with the playhead placed as of sending), `lastSeen` (Unix seconds, for one not linked), `wakeable` (the server can push to it), `owner` (for a device another account shares) and `acks`.

A device commands another with the report `{"type":"command","to":<device id>,"command":{…}}`; the server relays it if the target is the caller's, or shared with the caller. Only commands one device gives another are relayed: the server's own news (`devices`, `deviceKeys`, `shares`, `forgotten`, `historyChanged`) is refused, so a forged copy cannot pass as the server's. A command for a device that is not linked is kept and delivered when it links, and an iOS device is woken with a push if it gave a token and the server has a `[push]` key.

Other reports: `wake` (`to`, `notify`) asks the server to wake an unlinked device with a background push or, with `notify`, a notification to tap; `forget` (`device`) removes a device from the account, and every link is sent `{"type":"forgotten","device":…}`; `activity` (`token`, `device`, `sandbox`) registers a Live Activity push token showing another device, which the server updates as that device reports; `levels` (`f`) is a frame of the device's audio levels, sent while a controller has asked with `watchLevels` and relayed to it as `{"type":"levels","from":…,"f":…}` over a live link only.

`/rest/koanCommand?to=<device id>&command=<command JSON>` sends one command in a single request, for a device whose own link is down, such as an iPhone acting from its lock screen. `ack` optionally names the command's id (see `koanAck`).

### `koanAck`

Commands a device answers once it has acted on them, so the sender knows the command arrived rather than only that it was queued.

A command that wants an answer carries `"ack": <u64>` beside its fields, in the same JSON object. A device that linked with `acks=1` reports `{"type":"received","ack":…}` as soon as the command comes off its link, then `{"type":"ack","ack":…,"outcome":{…}}` once it has acted, where `outcome` is `{"result":"done"}`, `{"result":"refused","reason":…}` or `{"result":"failed","error":…}`. The server relays the answer to the sender as `{"type":"acked","from":<device>,"ack":…,"outcome":…}`, and itself answers `{"result":"queued"}` for a device that is away. A link that has gone quiet since a command was sent is dropped and the device woken.

Ids are random, and a device remembers the ids it has taken for a minute: a sender that hears nothing tries another route with the same id, and a repeat is answered with the first outcome and not acted on again.

`koanCommand` waits up to four seconds for the answer and returns it as a `koanCommand` element with `result`, and `reason` or `error`. With no answer in that time, the response is empty: sent.

### `koanShares`

Sharing a device with other accounts on the server. A device reports `{"type":"share","grantee":<username>,"allow":true|false}` about itself only. Its link is sent `{"type":"shares","grantees":[…],"accounts":[…],"error":…}` when it links and whenever the list changes, where `accounts` lists the server's accounts to choose from and `error` says why the last change was refused.

A shared device appears in each grantee's `devices` list with its `owner`. A grantee's command reaches it wrapped as `{"type":"shared","command":{…}}`, and runs as the grantee's request from the playback set: transport, the queue, the sleep timer, the volume, the output and preset, and moving the music. Nothing a shared command does changes the owner's library, favourites, playlists or history.

### `koanDeviceKeys`

Devices proving to each other on the local network that they belong to the same account, without the server in the room. A device that links with an API key and `deviceKey` registers its public key, kept on that key's row, so revoking the key or signing out drops it. A link signed any other way keeps no key.

Links that registered one are sent `{"type":"deviceKeys","keys":[{"id":…,"key":…,"owner":…}]}`: the account's devices and those shared with it, `owner` set for the latter. It is sent when such a device links and whenever the account's keys change. Only the server may send it.

## Account data

### `koanHistory`

The account's play history as the one record every device reads.

`/rest/koanHistory?since=<cursor>&count=<n>` (any role) answers `koanHistory` with `cursor`, `more`, a `play` per play after the cursor (`id`, `seq`, `played`, `listenedMs`) and a `forgotten` per play forgotten since (`id`, `played`). `count` defaults to 500 and is capped at 1000; omit `since` to start from the beginning. Page until `more` is false and keep the last `cursor`.

`/rest/koanForgetPlays` (role `user`) forgets plays for every device: repeated `id` with a matching `time` (when each play started, as `scrobble` takes it), or `through=<time>` for every play up to then.

Plays are recorded with `scrobble`. When the history moves, each linked device is sent `historyChanged`. See [Play history](../guide/remote-servers.md#play-history).

### `koanScrobbling`

The account's forwarding to ListenBrainz and Last.fm, set up from an app. Refused for the shared secret, which has no plays of its own.

| Endpoint | Parameters | Answer |
|----------|------------|--------|
| `koanScrobbling` | | `koanScrobbling`, one `service` per connected service with `name` (`listenbrainz`, `lastfm`), `account`, `connected`, `pending` (plays waiting), and `error` when the service stopped accepting the token |
| `koanScrobblingConnect` (**POST**) | `token`, in the body only | as `koanScrobbling`. The token is checked with ListenBrainz first |
| `koanScrobblingDisconnect` | `service` (`listenbrainz` by default, or `lastfm`) | as `koanScrobbling`; plays waiting are dropped |

The token is never sent back. Last.fm is connected through the web UI, which needs a browser round trip. See [Scrobbling](../guide/headless-server.md).

### `koanDspProfiles`

The account's EQ profiles kept on every device. Signed with the account's own password or an API key only.

| Endpoint | Parameters | Answer |
|----------|------------|--------|
| `koanDspProfiles` | `since` (revision; 0 for all) | `koanDspProfiles` with `cursor`, a `profile` per change after `since` (`uid`, `rev`, `editedAt`, and `doc`, absent for a deletion), and a `dismissed` per output whose AutoEQ suggestion was turned down (`output`) |
| `koanDspProfileSave` (**POST**) | `uid` (UUID), `editedAt`, `doc` (the profile's JSON) | `koanDspSaved` with `rev`, `stored`, and a `missing` (`sha256`) per file the profile names that the server lacks |
| `koanDspProfileDelete` (**POST**) | `uid`, `editedAt` | `koanDspSaved` |
| `koanDspFile` | `sha256` | the file's bytes |
| `koanDspFile` (**POST**) | `sha256`; the raw bytes as the body | empty |
| `koanDspDismiss` (**POST**) | `output` | empty |

The later edit wins: a save or delete whose `editedAt` is older than the stored copy's is not stored (`stored="false"`), and `editedAt` is capped at the server's clock so a device whose clock runs ahead cannot outrank later edits. After a stored save the client uploads each `missing` file; an upload must match its SHA-256 and the size the profile gives. The server caps the number of profiles and the total size of their files per account. Each stored change sends the account's links `dspProfilesChanged`. See [On every device](../guide/dsp.md#on-every-device).

### `koanDspDeleted`

Profiles deleted in the last thirty days, which the server keeps. The same credentials as `koanDspProfiles`.

`koanDspDeleted` answers `koanDspDeleted` with a `profile` per deletion, newest first: `uid`, `name`, `deletedAt`, `expiresAt`. `koanDspRestore` (**POST**, `uid`) saves one again as an edit made now, which every device adopts, and answers `koanDspSaved`. A profile whose files never reached the server cannot be restored.
