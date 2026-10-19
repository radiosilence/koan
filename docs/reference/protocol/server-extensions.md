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

## Sign-in and accounts

### `koanSignIn`

Trades the account's password for an API key, so a device sends the password once and keeps a key of its own.

`GET|POST /rest/koanSignIn?name=<device>`, signed with the account's own password (`p=enc:…` works over plain HTTP). Answers `join` with `username` and `apiKey`. The key replaces any of the account's keys with the same `name`, so a device that signs in again holds one key. Any other credential is refused with error 50.

### `koanInvite`

Invite links: an admin mints a signed token for an account, and a device trades it for an API key without the password. The token, the link format and redeeming it are on [Pairing and invites](pairing-and-invites.md#invites-koaninvite).

| Endpoint | Caller | Parameters | Answer |
|----------|--------|------------|--------|
| `koanJoin` | none: the token is the credential | `invite`, `name` | `join` with `username`, `apiKey` |
| `koanInvite` | admin | `username`, `reset=true` to set a new random password and disconnect the account's links | `invite` with `username`, `token`, and `password` when reset |
| `koanCreateUser` | admin | `username`, `role` (`admin`, `user`, `readonly`) | `invite` with `username`, `token`, `password` |
| `koanUsers` | admin | | `users`, one `user` per account with `username`, `role` |
| `koanSetUserRole` | admin | `username`, `role` | empty |
| `koanDeleteUser` | admin | `username`; not the caller's own | empty; the account's links are disconnected |
| `koanRevokeKey` | API key | | empty; revokes the key the request is signed with |

These are not listed as extensions of their own and are present wherever `koanInvite` is. The admin endpoints need an admin's own password or API key. `koanRevokeKey` is how an app gives up a key it has replaced, such as the one a join superseded.

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

Signing in a device with no keyboard: the device waits on a WebSocket at `/rest/koanPair` while someone signed in approves it with `koanPairInfo` and `koanPairApprove`. Specified on [Pairing and invites](pairing-and-invites.md#pairing-koanpair).

### `koanMcp`

`/rest/koanMcp` answers `mcp` with `url` (the server's `/mcp` endpoint) and `connect` (the page explaining how to connect an assistant). Listed only when `sharing.public_url` is set, since MCP's OAuth sign-in needs the address the server is reached at. See [MCP integration](../../guide/mcp-integration.md).

## Devices

### `koanLink`

The standing WebSocket at `/rest/koanLink` through which the server commands a client and the client reports what it is playing. Specified on [Link](link-protocol.md).

### `koanDevices`

The account's devices sent down each link, commands relayed between them, handoff, Live Activity pushes, and `/rest/koanCommand` for a device whose link is down. See [Link: Devices](link-protocol.md#devices). Each device's outputs travel with its state; a server older than the apps drops them, so the Output menu for another device needs the server updated too.

### `koanAck`

Commands answered once acted on, relayed back to the sender, with `koanCommand` waiting for the answer. See [Link: Acks](link-protocol.md#acks).

### `koanShares`

Sharing a device with other accounts on the server. See [Link: Shares](link-protocol.md#shares).

### `koanDeviceKeys`

Devices proving to each other on the local network that they belong to the same account. A device that links with an API key and `deviceKey` registers its public key, kept on that key's row, so revoking the key or signing out drops it. Links that registered one are sent `deviceKeys`, when they link and whenever the account's keys change. How the keys are used is on [Nearby](nearby-protocol.md#handshake).

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
