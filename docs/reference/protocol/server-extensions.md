# Server extensions

kōan's server speaks Subsonic, and adds what Subsonic has no way to say: a server reaching a client, devices commanding each other, signing in without typing a password into every device, and per-account state such as play history and EQ that every device shares. Each addition is listed as an OpenSubsonic extension in `getOpenSubsonicExtensions`, and a client decides what to offer from that list rather than from the server calling itself kōan. A kōan server older than an extension does not list it, and a server that is not kōan may list one.

This page specifies each extension: what it is for, its endpoints, and who may call them. The link's messages are on [Link](link-protocol.md). kōan's own clients implement the client side in `koan_core::remote`.

## Conventions

Unless an endpoint says otherwise:

- Every endpoint is under `/rest/` and authenticates like any other Subsonic call: `u` with `p` or `t`+`s`, or `apiKey`. `f=json` returns JSON, otherwise XML, inside the usual `subsonic-response` envelope.
- Parameters may be sent in the query string or, under OpenSubsonic's `formPost`, in an `application/x-www-form-urlencoded` POST body (except a `koanDspFile` upload, whose body is the file). Endpoints marked **POST** accept only POST; secrets they take belong in the body, where proxies do not log them.
- Times are milliseconds since the Unix epoch, except where an attribute is an ISO 8601 string.
- Track ids are the server's track uids, the ids `getSong` and the rest publish.
- An answer with no element named is an empty `ok` response.

Credentials differ in what they may do. An account's **own password** and its **API keys** may manage the account's credentials and its EQ. An **app password** is a credential handed to one third-party app, and the server's **shared secret** (`[subsonic]`) belongs to no account; neither may make keys, approve pairings or reach EQ. See [Authentication](../../guide/authentication.md).

## The server's address

`ping` from a kōan server with `server.url` set carries `serverUrl`, the address it is reached at, beside OpenSubsonic's `type` and `serverVersion`. A client keeps it while signed in and identifies the server by it, as the `library` it announces on the local network, so devices that reach one server at different addresses agree that it is the same one. A server without it, or one that is not kōan, sends none, and the client falls back to the address it signed in with.

## Sign-in and accounts

### `koanSignIn`

Trades the account's password for an API key, so a device sends the password once and keeps a key of its own.

`GET|POST /rest/koanSignIn?name=<device>`, signed with the account's own password (`p=enc:…` works over plain HTTP). Answers `join` with `username` and `apiKey`. The key replaces any of the account's keys with the same `name`, so a device that signs in again holds one key. Any other credential is refused with error 50.

### `koanInvite`

Invite links: an admin mints a signed token for an account, or any account for itself, and a device trades it for an API key without the password. The token, the link format and redeeming it are on [Pairing and invites](pairing-and-invites.md#invites-koaninvite).

| Endpoint | Caller | Parameters | Answer |
|----------|--------|------------|--------|
| `koanJoin` | none: the token is the credential | `invite`, `name` | `join` with `username`, `apiKey` |
| `koanInvite` | any account for its own `username`; an admin for any, and for `reset` | `username`, `reset=true` to set a new random password and disconnect the account's links | `invite` with `username`, `token`, `qrToken`, and `password` when reset |
| `koanCreateUser` | admin | `username`, `role` (`admin`, `user`, `readonly`) | `invite` with `username`, `token`, `qrToken`, `password` |
| `koanUsers` | admin | | `users`, one `user` per account with `username`, `role` |
| `koanSetUserRole` | admin | `username`, `role` | empty |
| `koanDeleteUser` | admin | `username`; not the caller's own | empty; the account's links are disconnected |
| `koanRevokeKey` | API key | | empty; revokes the key the request is signed with |

These are not listed as extensions of their own and are present wherever `koanInvite` is. The admin endpoints, and `koanInvite` for oneself, need the account's own password or API key. `koanRevokeKey` is how an app gives up a key it has replaced, such as the one a join superseded.

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
| `koanAppPasswords` | | `appPasswords`, one `appPassword` per password with `id`, `name`, `created`, `lastUsed` (ISO 8601). Never the password |
| `koanCreateAppPassword` (**POST**) | `name` | `appPassword` with `id`, `name`, `password`. Shown once; error 0 when the account holds as many as it may |
| `koanRevokeAppPassword` (**POST**) | `id` | empty |

### `koanPair`

Signing in a device with no keyboard: the device waits on a WebSocket at `/rest/koanPair` while someone signed in approves it with `koanPairInfo` and `koanPairApprove`. Specified on [Pairing and invites](pairing-and-invites.md#pairing-koanpair).

### `koanMcp`

`/rest/koanMcp` answers `mcp` with `url` (the server's `/mcp` endpoint) and `connect` (the page explaining how to connect an assistant). Listed only when `server.url` is set, since MCP's OAuth sign-in needs the address the server is reached at. See [MCP integration](../../guide/mcp-integration.md).

## Devices

### `koanLink`

The standing WebSocket at `/rest/koanLink` through which the server commands a client and the client reports what it is playing. Specified on [Link](link-protocol.md).

### `koanDevices`

The account's devices sent down each link, commands relayed between them, handoff, Live Activity pushes, and `/rest/koanCommand` for a device whose link is down. See [Link: Devices](link-protocol.md#devices).

### `koanPush`

The server pushes to the iOS app through kōan's [push relay](push-relay.md): wakes it once iOS has suspended it, asks it to play, updates its Live Activities. Listed only where `[push]` is on. Where it is listed, the app asks to show notifications, registers with the relay and sends the relay's handles up the link (`push`, `activity`), and runs a tapped notification's command by sending its `claim`. Where it is not, the app asks for nothing and sends the server nothing.

### `koanAck`

Commands answered once acted on, relayed back to the sender, with `koanCommand` waiting for the answer. See [Link: Acks](link-protocol.md#acks).

### `koanShares`

Sharing a device with other accounts on the server. See [Link: Shares](link-protocol.md#shares).

### `koanDeviceKeys`

Devices proving to each other on the local network that they belong to the same account. A device that links with an API key and `deviceKey` registers its public key, kept on that key's row, so revoking the key or signing out drops it. A key binds to the first device it registers. A key that has registered no device yet, as one made by signing in again, takes the device over, and the account's older keys for that device are revoked. Any link opened with a `deviceKey` parameter is sent `deviceKeys`, when it links and whenever the account's keys change, whether or not its key could be kept. How the keys are used is on [Nearby](nearby-protocol.md#handshake).

### `koanTvPairing`

The server keeps the account's Apple TV pairings and hands them to the account's devices, which wake the TVs with them. See [Link: Apple TV pairing](link-protocol.md#apple-tv-pairing).

## Account data

### `koanHistory`

The account's play history as the one record every device reads.

`/rest/koanHistory?since=<cursor>&count=<n>` (any role) answers `koanHistory` with `cursor`, `more`, a `play` per play after the cursor (`id`, `seq`, `played`, `listenedMs`) and a `forgotten` per play forgotten since (`played`, and `id` unless the track has left the library). `count` defaults to 500 and is capped at 1000; omit `since` to start from the beginning. Page until `more` is false and keep the last `cursor`.

`/rest/koanForgetPlays` (role `user`) forgets plays for every device: repeated `id` with a matching `time` (when each play started, as `scrobble` takes it), or `through=<time>` for every play up to then.

Plays are recorded with `scrobble`. When the history moves, each linked device is sent `historyChanged`. See [Play history](../../guide/remote-servers.md#play-history).

### `koanScrobbling`

The account's forwarding to ListenBrainz and Last.fm, set up from an app. Refused for the shared secret, which has no plays of its own.

| Endpoint | Parameters | Answer |
|----------|------------|--------|
| `koanScrobbling` | | `koanScrobbling`, one `service` per connected service with `name` (`listenbrainz`, `lastfm`), `account`, `connected` (when, in ms), `pending` (plays waiting), and `error` when the service stopped accepting the token |
| `koanScrobblingConnect` (**POST**) | `token`, in the body only | as `koanScrobbling`. The token is checked with ListenBrainz first |
| `koanScrobblingDisconnect` | `service` (`listenbrainz` by default, or `lastfm`) | as `koanScrobbling`; plays waiting are dropped |

The token is never sent back. Last.fm is connected through the web UI, which needs a browser round trip. See [Scrobbling](../../guide/headless-server.md#scrobbling).

### `koanPlaylistOrder`

The order of the account's playlists, which Subsonic has no call for, kept on the server so every device lists them alike.

`getPlaylists` lists the account's playlists in that order. `/rest/koanPlaylistOrder` (role `user`) sets it from repeated `id`, the playlist ids in the order wanted, and answers empty. The client sends the whole list on every reorder, so two devices reordering at once settle on one list rather than a mix of both. The account's own playlists are ordered; another account's public playlist named in the list keeps the place its owner gave it. Ids the server does not know are skipped, and the account's playlists the list leaves out keep their places between those it names. Each change sends the account's links `sync`.

A client adopts the listing's order when it syncs playlists, leaving its playlists the server does not hold (local ones, ones not yet pushed) where they sit among the rest. A reorder made on the client before its last send succeeded is sent at the next sync instead of being replaced by the server's order. A server without the extension lists playlists in its own order, which clients do not adopt.

### `koanDspProfiles`

The account's EQ profiles kept on every device. Signed with the account's own password or an API key only.

| Endpoint | Parameters | Answer |
|----------|------------|--------|
| `koanDspProfiles` | `since` (revision; 0 for all) | `koanDspProfiles` with `cursor`, a `profile` per change after `since` (`uid`, `rev`, `editedAt`, and `doc`, absent for a deletion), and a `dismissed` per output whose AutoEQ suggestion was turned down (`output`) |
| `koanDspProfileSave` (**POST**) | `uid` (UUID), `editedAt`, `doc` (the profile's JSON) | `koanDspSaved` with `rev`, `stored`, and a `missing` (`sha256`) per file the profile names that the server lacks |
| `koanDspProfileDelete` (**POST**) | `uid`, `editedAt` | `koanDspSaved` |
| `koanDspFile` | `sha256` | the file's bytes |
| `koanDspFile` (**POST**) | `sha256` and the credentials in the query string; the raw bytes as the body, not form-encoded | empty |
| `koanDspDismiss` (**POST**) | `output` | empty |

The later edit wins: a save or delete whose `editedAt` is older than the stored copy's is not stored (`stored="false"`), and `editedAt` is capped at the server's clock so a device whose clock runs ahead cannot outrank later edits. After a stored save the client uploads each `missing` file; an upload must match its SHA-256 and the size the profile gives. The server caps the number of profiles and the total size of their files per account. Each stored change sends the account's links `dspProfilesChanged`. See [On every device](../../guide/dsp.md#on-every-device).

### `koanDspDeleted`

Profiles deleted in the last thirty days, which the server keeps. The same credentials as `koanDspProfiles`.

`koanDspDeleted` answers `koanDspDeleted` with a `profile` per deletion, newest first: `uid`, `name`, `deletedAt`, `expiresAt`. `koanDspRestore` (**POST**, `uid`) saves one again as an edit made now, which every device adopts, and answers `koanDspSaved`. A profile whose files never reached the server cannot be restored.

## Library

### `koanCovers`

Many records' covers in one request, for a client keeping a library's covers on the device ahead of time. A cover fetched with `getCoverArt` costs a round trip each; this one costs a round trip per two hundred, and the server answers from the covers it has already made, reading files and decoding nothing.

`/rest/koanCovers` (role `user`) takes repeated `id`, the cover ids the library's albums carry (`coverArt`) or a track's id, which stands for its record, at most 200, and `size`, snapped as `getCoverArt` snaps it. It answers `application/vnd.koan.covers`: one frame per id, in the order asked, each

| Bytes | |
|-------|---|
| 2 | the id's length, big-endian |
| that many | the id as asked, UTF-8 |
| 1 | `0` a cover, `1` the record has no cover, `2` not held |
| 4 | the cover's length, big-endian; 0 unless it is a cover |
| that many | the cover, JPEG |

A cover is answered only where the server holds it ready at that size; it warms every album at 300 and 600. Anything else, an id that is neither an album's nor a track's, one it does not know, or an album whose cover it has not made, is `2`, and the client asks `getCoverArt` for it as before. A client reads a kind it does not know as `2`, so later kinds can be added. A client refuses a frame claiming a cover over 32 MB, and asks one by one for every id the stream did not answer, however it ended. A refusal (sign-in, too many ids, none) is the usual `subsonic-response` document instead of the stream.

kōan's apps use it for the cover preload when the server lists it, two hundred ids at a time, reading each answer only while the preload may go on, and fetch the rest, or every cover from a server without it, six at a time with `getCoverArt`. The preload starts a request only while none of a page's covers is in flight, pauses reading an answer while one is, and never delays a page's requests. A stream cut part way is asked once more for the ids it did not reach; a server that refuses or is unavailable is not asked again, and those ids go to `getCoverArt`. The server reads a few covers ahead of the frame it is sending and makes the rest as the answer is read, so a client reading slowly, or gone, holds little on the server.

