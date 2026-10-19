# Nearby

kōan apps on the same network see and control each other directly, with no server between them and whichever account each is signed in as. A nearby connection carries the same messages as the [link](link-protocol.md); this page covers what differs: discovery, the handshake, how a peer is trusted, and what it may send.

## Discovery

An app that is discoverable listens on `devices.port` (default 5626, "koan" on a phone keypad) and announces itself as `_koan._tcp` under its device name. The port is fixed so that addresses typed into other devices keep working; if it is taken, the app binds an ephemeral port and announces that.

| TXT key | Value |
|---------|-------|
| `id` | The device's id, the same id it links to its server with |
| `platform` | `macos`, `ios`, `tvos`, `android`, `linux` |
| `server` | The signed-in server's address, without a trailing `/`. Only when signed in, for an `http(s)` address without credentials, query or fragment that is not loopback, and only if `server=<address>` fits the 255-byte TXT entry |

The Apple apps register and browse with `dns_sd`; the Android app with `NsdManager`. The terminal UI has no discovery and dials only addresses. An app announces itself only while it listens, which needs `devices.nearby` and `devices.discoverable` on, and on iOS the app to be findable (see `quiet.rs`).

Each app dials every device it finds, and every address in `devices.addresses`, which is how a tailnet without Bonjour is reached. A bare host is dialled on port 5626. A device is known by its `id`; an announcement with no `id` is known by its service name, and a typed address by itself. Failed dials back off from 2 to 60 seconds.

## Transport

A WebSocket over plain TCP, at path `/`, with no query. Each text frame holds one JSON object; binary and control frames only count as liveness. A connection silent for 45 seconds is pinged, and hung up after another 45 without an answer. The TCP connect times out after 3 seconds and the WebSocket handshake after 10.

The listening end reports and the dialling end commands.

## Handshake

1. The listener sends `hello`:

   ```json
   {"type":"hello","id":"…","name":"Living room","platform":"macos",
    "library":"5d41402abc4b2a76b9719d911017c592","acks":true,"nonce":"reports.…"}
   ```

   `library` is the hex MD5 of the signed-in server's address, lowercased and without a trailing `/`, or `null` when signed out. Two devices with the same `library` share track ids, so music can move between them. `acks` says the listener answers commands sent with an id ([acks](link-protocol.md#acks)). `nonce` is base64 of 32 random bytes; prefixed with `reports.`, it also says the listener signs its reports.

2. A dialler that is signed in with an API key, and so holds a device keypair, proves itself with `nearbyAuth`: `{"type":"nearbyAuth","id":…,"nonce":…,"sig":…}`. It sends no commands until answered.

3. The listener checks the signature against the device keys its server sent it (`koanDeviceKeys`) and answers `{"type":"nearbyProof","verified":true|false,"sig":…}`, its own signature present when it holds a key. Only the first `nearbyAuth` is answered.

A peer that proves nothing, because it is older, signed out or on another server, carries on unproven.

### Signatures

Ed25519, base64 (standard alphabet). Each signed message is the label followed by the fields, every one of them, the label included, written as its length (a 32-bit big-endian integer) followed by its UTF-8 bytes. The transcript is built the same way.

| Signature | Label | Fields |
|-----------|-------|--------|
| Dialler | `koan-nearby-v1 dial` | listener id, dialler id, listener nonce, dialler nonce |
| Listener | `koan-nearby-v1 listen` | dialler id, listener id, dialler nonce, listener nonce, one byte: `verified` |
| Command | `koan-nearby-v1 command` | transcript, `seq` as a 64-bit big-endian integer, the command's JSON |
| Report | `koan-nearby-v1 report` | transcript, `seq`, the report's JSON |

The transcript is the SHA-256 of the message with label `koan-nearby-v1 session` and fields listener id, dialler id, listener nonce, dialler nonce. Both handshake signatures name both devices, both nonces and which end made them, so a recorded handshake fails against a fresh nonce.

A key with no `owner` in `deviceKeys` proves the account's own device; one with an `owner` proves a device shared by that account. A kept key list older than thirty days is not trusted.

### Signed traffic

Nearby connections are not encrypted, so after a verified handshake every command goes as `{"type":"nearbySigned","seq":n,"sig":…,"command":"<JSON>"}`, and the listener refuses unsigned ones. Reports go as `{"type":"nearbySignedReport","seq":n,"sig":…,"report":"<JSON>"}` when the listener signed and both nonces carry the `reports.` mark; the listener then resends its state signed. `seq` starts at 1 on each side, and a receiver takes only a `seq` above the last it accepted, so a command slipped in, replayed or taken from another session is refused.

## What a peer may do

| Peer | How it is known | What it may send |
|------|-----------------|------------------|
| The account's own device | Proven with a key that has no `owner` | Every command a device may relay through the server (`LinkCommand::relayable`), and `watchLevels` |
| A device shared with this account | Proven with an `owner`'s key | The playback set |
| Anyone else, under **Full control** (`devices.nearby_control = "full"`, the default) | Unproven | The playback set |
| Anyone else, under **Playback only** (`"playback"`) | Unproven | The stranger's set |

`watchLevels` is taken from every peer, whatever its class: levels say no more than the state it already sees.

The **playback set** (`LinkCommand::allowed_playback`): `play`, `enqueue`, `playNext`, `remove`, `clear`, `jumpTo`, `seek`, `pause`, `resume`, `next`, `previous`, `playItem`, `removeItems`, `moveItems`, `insert`, `undo`, `redo`, `shuffle`, `repeat`, `sleepTimer`, `handOff`, `setOutput`, `refreshOutputs`, `setRendererVolume`, `setPreset`, `watchLevels`.

The **stranger's set** (`LinkCommand::allowed_nearby`) is the playback set without `setOutput`, `refreshOutputs`, `setRendererVolume` and `setPreset`: where the sound goes reaches into the room and the config. Under Playback only, the listener also leaves `outputs` out of its state.

Neither set includes anything that touches the library, the files on disk, or the account's favourites, playlists or history, and none of the server's news (`devices`, `deviceKeys`, `shares`, `forgotten`, `historyChanged`, `dspProfilesChanged`) is taken from the network. A refused command sent with an `ack` is answered `{"result":"refused","reason":"not allowed from this network"}`.

A device's `id` is its own claim. When the `id` in a listener's `hello` belongs to one of the account's devices, or one shared with it (as the server's `devices` list names them), the dialler sends it commands and acts on its reports only once it has proved it is that device under that account; a device nobody lists is taken at its word as a stranger. Addresses in `devices.refused` are hung up on at once, and Disconnect in Settings holds a device off by address, and by id once it has proven itself.

## Reports

The listener sends `state` whenever it changes beyond the playhead moving, `levels` while watched, and `ack` for commands sent with an id, all as on the link. A dialler that finds its own `id` in a `hello` stops dialling that address, and a second connection to an id already connected is hung up as a duplicate.
