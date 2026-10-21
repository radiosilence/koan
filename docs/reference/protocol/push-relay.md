# Push relay

Apple delivers pushes to the kōan iOS app only when they are signed with a key belonging to the developer's team, so a self-hosted kōan server cannot wake a suspended iPhone or ask it to play on its own. The push relay at `push.koan.rocks` holds a topic-specific key of its own and forwards pushes for any server.

The key belongs to the developer's Apple account, which answers to Apple for every push sent under it, so the relay is built to bound what anyone can cause through it. The worst a hostile server can do is make a phone show kōan's own text, within small quotas, to people who chose to sign in to it. Nobody can make the relay send Apple a flood, a stream of errors, or anything a server wrote.

That bound holds only together with the app: its notification service extension shows nothing a server wrote, only its own template (see [What a phone shows](#what-a-phone-shows)).

The relay does not serve real traffic until registration also requires an [App Attest](https://developer.apple.com/documentation/devicecheck/establishing-your-app-s-integrity) assertion (#1418). Until then, made-up tokens are bounded by rate, as below, but not by who asks.

The relay is built, released and deployed from its own repository, [radiosilence/koan-push-relay](https://github.com/radiosilence/koan-push-relay), which also says how to run it. This page specifies the API kōan servers and the app use.

## Parties

- **The app** holds its APNs device token. It registers the token with the relay once for each server it uses, gives each server the handle it gets back, and keeps the revoker for itself. It gives the raw token to no server except one that pushes with the developer's own key (`koanApns`).
- **A server** holds the handles its users' apps gave it, and asks the relay to push to them.
- **The relay** opens handles, applies quotas and writes everything Apple is sent, apart from an encrypted blob a server may attach to a notification.

## Handles

A handle is the device token, the gateway (production or sandbox), the time it was minted and a slot, sealed with AES-256-GCM-SIV under a key only the relay holds, and encoded as base64url without padding:

```text
version (1) | key id (1) | nonce (12) | sealed( flags (1) | minted ms (8, big-endian) | slot (16) | token ) | tag (16)
```

The version and key id are authenticated with the rest. A handle is opaque to everyone but the relay: clients store it and send it back unchanged, and must not parse it. It is at most 208 characters.

The **slot** is sixteen random bytes the app chooses for one server and keeps for it, across renewals, for as long as it uses that server. The relay counts that server's allowance for the device by it, so a renewed handle shares the old one's allowance, and stamps it on every notification it sends (`koan-from`), so the app knows which server a notification came through without taking any server's word for it.

A handle opens for 60 days from minting by default, and at most 365 (`expires` in the registration response). The app registers again at half its lifetime and gives the server the new handle on its next link. A handle minted more than a lifetime ahead of the relay's clock is answered `503`, not `410`: the clock is wrong, not the handle.

Registration also returns a **revoker**: the same sealed form with a flag set, kept by the app and given to no server. A revoker cannot be pushed through, and revoking takes one, so a server, which holds handles alone, cannot revoke a device.

The relay keeps no record of handles. It can rotate its sealing key: handles sealed under a key it still lists keep opening, and a key is retired once every handle sealed under it has expired.

## Revocation

A device revokes by sending a revoker from any of its registrations that has not expired. Revoking asks nothing of Apple: the relay minted the revoker only for a token Apple had accepted. Every handle for that token, and every revoker, minted at or before that moment stops opening, and so does any minted in the ten minutes after it, so that a clock stepped back by up to that much cannot bring a revoked handle back. A registration after a revocation is minted past that window. If the relay's clock has read later than it does now, the revocation is recorded at that later time instead, so a handle minted while the clock ran fast cannot outrun it; and a device's revocation is always recorded after its previous one, so it reaches a handle minted past that one's window.

The app revokes when it signs out of a server, then registers again, gives each server it still uses a fresh handle, and keeps the fresh revoker. It waits for the revocation's `204` before registering again, and registers again after any revocation that succeeds, including a retried one: a revocation retires every handle minted before it reached the relay, whichever request asked for them. A retried revocation answered `410` means the first one landed and retired the revoker with the rest; the app treats it as done and registers again.

When Apple answers a push that the app is no longer on the device (`410`), or refuses a token it accepted at registration as no longer valid (`400 BadDeviceToken` or `DeviceTokenNotForTopic`), the relay records a revocation for that device itself, so a server that ignores the `410` cannot keep sending to it. Pushes to the device already on their way when the first answer comes add nothing, and further pushes stop at the relay.

The relay stores one row per revoking or gone device: a keyed hash of the token and the time. A row is kept for 365 days, the longest a handle can be configured to open, measured from the newest row rather than the clock if the clock reads later, so neither a change of lifetime nor a clock jumped ahead lets a revoked handle open again.

## Proof of a device

Before minting a handle, the relay sends the token a background push (`wake-now`) and goes on only if Apple accepts it, so no handle can be had for a made-up token. The app is open when it registers, so the wake asks nothing of it.

Registration is the one place a made-up token reaches Apple, so proofs have limits of their own:

- a rate for everyone;
- a count, per caller address, of tokens Apple refused: past its limit, that address proves nothing until the count falls back;
- the same count across everyone, past which no registration is proved until it falls back.

Room in both counts is taken before a proof is sent and handed back if Apple accepts the token, so racing proofs cannot overrun them. A proof also counts against the device's wakes. Revocation asks nothing of Apple, so none of this can stop a device revoking.

## Endpoints

All requests are `POST` with a JSON body of at most 8 KiB, and `Content-Type: application/json`; anything else is refused with `415` before it is read. That keeps a web page from making visitors' browsers call the relay: a cross-origin JSON request needs a CORS preflight, which the relay never answers. Unknown fields are refused with `400`, unlike the link protocol: the relay accepts nothing it does not write itself. A body that takes longer than five seconds to arrive is answered `408`, and one too large `413`. Errors have the body `{"error": "<reason>"}`; the reason is for logs, and clients act on the status alone.

### `POST /v1/register`

Called by the app.

```json
{ "token": "<device token, hex>", "sandbox": false, "slot": "<16 bytes, hex>" }
```

`sandbox` is true for development builds, whose tokens only Apple's sandbox gateway accepts; it may be absent. The answer is `200`:

```json
{ "handle": "<handle>", "expires": 1767225600 }
```

`expires` is in Unix seconds. `400` means the token is not 8 to 100 bytes of hex, the slot is not 16 bytes of hex, the relay holds no key for the requested gateway, or Apple refused the token; `429` and `503` are as for a push.

The answer also carries `"revoker": "<revoker>"`, for the app alone.

### `POST /v1/revoke`

Called by the app.

```json
{ "revoker": "<revoker>" }
```

The answer is `204` once the revocation is on disk. `400` means what was sent is a handle, not a revoker; `410` that the revoker does not open (unknown, altered, expired, or already revoked); `429` and `503` are as for a push.

### `POST /v1/push`

Called by servers. There is no other authentication: the handle is the capability, and quotas bound what holding one is worth.

```json
{ "handle": "<handle>", "kind": "notify", "blob": "<base64url>" }
```

| `kind` | Apple push | What the phone gets |
|--------|------------|---------------------|
| `wake` | background, low priority, collapse id `koan-wake`, expires in an hour | `content-available` and nothing else: the app wakes, links and takes what waits for it |
| `wake-now` | as `wake`, collapse id `koan-wake-now`, expires in a minute | the same, for a wake that is no use late |
| `summon` | alert, collapse id `koan-summon-<slot>`, expires in two minutes | the relay's notification |
| `notify` | alert, collapse id `koan-play-<slot>`, expires in ten minutes | the relay's notification |

A notification's collapse id carries its server's slot, so one server's notification replaces only that server's last one.

The relay's notification is titled "kōan", reads "Something is waiting to play. Tap to open.", plays the default sound and has the interruption level `active`: never `time-sensitive`, since the relay cannot vouch for the server, so it does not break through a Focus. It carries the slot as `koan-from`. A `summon` or `notify` may carry `blob`, at most 3072 base64url characters, which the relay passes to the app under `koan-e2e` with `mutable-content` set. A `wake` or `wake-now` with a blob is refused with `400`. The relay does not read the blob.

| Status | Meaning | What the server does |
|--------|---------|----------------------|
| `204` | Apple accepted the push | |
| `400` | The request is malformed | Fix the request |
| `410` | The handle does not open (unknown, altered, expired, revoked), the relay no longer serves its gateway, or Apple says the app is gone from the device | Forget the handle; the app sends a new one when it next links |
| `429` | A quota is spent, or Apple is throttling the device | Wait at least `Retry-After` seconds |
| `503` | The relay or Apple cannot take the push now, Apple refused it for a reason the relay does not know, or the relay has stopped sending because Apple refuses it (`Retry-After` is then minutes) | Retry after `Retry-After` seconds |

A push whose sender hangs up once it has been accepted for sending still goes to Apple, and is counted as sent.

### What a phone shows

The extension decrypts the blob, but decrypting proves only that the sender holds the device's public key, which every server the person signed in to does. So the extension shows nothing a server wrote:

- It names the server from `koan-from`, the slot the relay stamped, mapped to the server the app gave that slot, never from anything in the blob. Cover art is fetched only from that server's own origin, must be an image, and is capped in size.
- The blob carries structured fields, never display text: the command, and the title and artist of what it would play. The extension fills its own template with them, capped in length and stripped of links, phone numbers, addresses and control characters. A title or body in the blob is ignored.
- Tapping runs only playback commands.
- When the blob does not decrypt, or `koan-from` names no server the app knows, the relay's text stands and tapping opens the app.

Live Activity updates are not relayed. ActivityKit renders their content without passing it through the app, so they cannot be encrypted or constrained.

## Quotas

Each relay process counts:

| Counted per | Default |
|-------------|---------|
| Server's handle: notifications | 6 an hour |
| Server's handle: wakes | 12 an hour |
| Device, across its servers: notifications | 20 an hour |
| Device, across its servers: wakes, proofs of registration included | 30 an hour |
| Device: revocations | 10 an hour |
| Caller address: pushes | 6000 a minute |
| Caller address: registrations | 120 a minute |
| Caller address: revocations | 60 a minute |
| Caller address: tokens Apple refuses in proofs | 10 an hour |
| Everyone: tokens Apple refuses in proofs, before registrations are held | 360 an hour |
| Everyone: proofs sent to Apple | 20 a second |
| Everyone: pushes sent to Apple | 20,000 a second |

A server's handle is counted by device and slot, so its renewals share it. A device is counted by its token, so its ceiling holds whichever servers hold its handles. A caller address is counted by its IPv4 address, or its IPv6 /64 for pushes and /48 for registrations, revocations and refused tokens: the larger block one site is commonly given, so a site cannot spread made-up tokens over its addresses, at the price that neighbours in a /48 share those allowances. Each count allows its whole allowance as a burst and then refills evenly over the period. A push the relay or Apple could not take is not counted against its server or device.

Behind proxies, the address is read from the header the relay is configured to trust (`client_ip_header`), across all its lines, `client_ip_hops` addresses back from the end. A request without a readable address there is refused with `400`, rather than counted as the proxy's own address, where every such caller would share one allowance. The relay must be reachable only through the proxy that writes that header, since anyone reaching it directly can write it themselves.

## When Apple refuses the relay

When Apple refuses a gateway's credential (an invalid, missing or over-refreshed signing key, a wrong key id, topic or environment, an expired signing key signed only moments ago, which means the relay's clock is wrong, or tokens proven at registration refused for five different devices on that gateway within ten minutes), every push through that gateway would be refused the same way, and sending on with a credential Apple has refused is what gets a key looked at. That gateway stops: its pushes and proofs are answered `503` with `Retry-After: 300` without reaching Apple, and one a minute is let through to learn whether it has been fixed. It opens again only when Apple answers in a way only a working credential gets; the other gateway carries on meanwhile. An answer the relay does not recognise is refused with `503` and `Retry-After: 300` but does not stop the gateway. The relay never signs a new provider token within twenty minutes of the last.

## What the relay sees and keeps

It sees, per request: a device token (registration and revocation only), a handle, which it opens to a token in memory, the kind of push, an encrypted blob it cannot read, and the caller's address.

It keeps:

- the revocation table: a keyed hash of each revoking or gone device's token and when, for 365 days, and the latest time its clock has read;
- counters for quotas, in memory: keyed hashes of tokens, slots, and caller addresses as they are (IPv6 cut to its prefix), each forgotten once its allowance has refilled;
- aggregate counts by route and outcome, pushes and proofs.

It writes no request log, and nothing it writes to disk or to its own log names a token, a handle, a caller or a server. Its own log level is `KOAN_RELAY_LOG` (`info` by default); every library it uses logs at `warn` whatever `RUST_LOG` says, since at `debug` the HTTP libraries log request URLs, and Apple's hold device tokens.
