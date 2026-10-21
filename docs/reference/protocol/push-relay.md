# Push relay

Apple delivers pushes to the kōan iOS app only when they are signed with a key belonging to the developer's team, so a self-hosted kōan server cannot wake a suspended iPhone or ask it to play on its own. The push relay at `push.koan.rocks` holds a topic-specific key of its own and forwards pushes for any server.

The key belongs to the developer's Apple account, which answers to Apple for every push sent under it, so the relay is built to bound what anyone can cause through it. The worst a hostile server can do is make a phone show kōan's own text, within small quotas, to people who chose to sign in to it. Nobody can make the relay send Apple a flood, a stream of errors, or anything a server wrote.

That bound holds only together with the app: its notification service extension shows nothing a server wrote, only its own template (see [What a phone shows](#what-a-phone-shows)).

The relay is `crates/koan-push-relay`. This page specifies its API; running it is under [Operating the relay](#operating-the-relay).

## Parties

- **The app** holds its APNs device token. It registers the token with the relay once for each server it uses, and gives each server the handle it gets back. It gives the raw token to no server except one that pushes with the developer's own key (`koanApns`).
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

The relay keeps no record of handles. It can rotate its sealing key: handles sealed under a key it still lists keep opening, and a key is retired once every handle sealed under it has expired.

## Revocation

A device revokes by sending its token. Every handle for that token minted at or before that moment stops opening, and so does any minted in the ten minutes after it, so that a clock stepped back by up to that much cannot bring a revoked handle back. A registration after a revocation is minted past that window. If the relay has minted a handle at a time later than its clock now reads, the revocation is recorded at that time instead, so a handle minted while the clock ran fast cannot outrun it.

The app revokes when it signs out of a server, then registers again and gives each server it still uses a fresh handle. It waits for the revocation's `204` before registering again, and registers again after any revocation that succeeds, including a retried one: a revocation retires every handle minted before it reached the relay, whichever request asked for them.

When Apple answers a push that the app is no longer on the device (`410`), the relay records a revocation for that device itself, so a server that ignores the `410` cannot keep sending to it.

The relay stores one row per revoking or gone device: a keyed hash of the token and the time. A row is kept for 365 days, the longest a handle can be configured to open, measured from the newest row rather than the clock if the clock reads later, so neither a change of lifetime nor a clock jumped ahead lets a revoked handle open again.

## Proof of a device

Before minting a handle or recording a revocation, the relay sends the token a background push (`wake-now`) and goes on only if Apple accepts it. Handles and revocations therefore exist only for real devices: neither can be had for a made-up token. The app is open when it registers or revokes, so the wake asks nothing of it.

Proofs are the one place a made-up token reaches Apple, so they have limits of their own:

- a rate for everyone;
- a count, per caller address, of tokens Apple refused: past its limit, that address proves nothing until the count falls back;
- the same count across everyone, which holds registrations, never revocations, past its limit, so no one else's made-up tokens can stop a device revoking.

A proof is counted against the device's allowance before it is sent: a registration's against its wakes, a revocation's against a separate per-device allowance, so a server that spends a device's wakes cannot stop it revoking.

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

### `POST /v1/revoke`

Called by the app.

```json
{ "token": "<device token, hex>", "sandbox": false }
```

The answer is `204` once the revocation is on disk. `400` means the token is malformed, the relay holds no key for the gateway, or Apple refused the token; `429` and `503` are as for a push.

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
| `503` | The relay or Apple cannot take the push now, or the relay has stopped sending because Apple refuses it (`Retry-After` is then minutes) | Retry after `Retry-After` seconds |

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
| Everyone: tokens Apple refuses in proofs, before registrations are held | 3600 an hour |
| Everyone: proofs sent to Apple | 20 a second |
| Everyone: pushes sent to Apple | 20,000 a second |

A server's handle is counted by device and slot, so its renewals share it. A device is counted by its token, so its ceiling holds whichever servers hold its handles. A caller address is counted by its IPv4 address, or its IPv6 /64 for pushes and /48 for registrations, revocations and refused tokens: the larger block one site is commonly given, so a site cannot spread made-up tokens over its addresses, at the price that neighbours in a /48 share those allowances. Each count allows its whole allowance as a burst and then refills evenly over the period. A push the relay or Apple could not take is not counted against its server or device.

Behind proxies, the address is read from the header the relay is configured to trust (`client_ip_header`), across all its lines, `client_ip_hops` addresses back from the end. The relay must be reachable only through the proxy that writes that header, since anyone reaching it directly can write it themselves.

## When Apple refuses the relay

When Apple refuses a gateway's credential (an invalid or over-refreshed signing key, a wrong topic or environment, or a token proven at registration now refused as not this app's), every push through that gateway would be refused the same way, and sending on with a credential Apple has refused is what gets a key looked at. That gateway stops: its pushes and proofs are answered `503` with `Retry-After: 300` without reaching Apple, and one a minute is let through to learn whether it has been fixed. It opens again only when Apple answers in a way only a working credential gets; the other gateway carries on meanwhile.

## What the relay sees and keeps

It sees, per request: a device token (registration and revocation only), a handle, which it opens to a token in memory, the kind of push, an encrypted blob it cannot read, and the caller's address.

It keeps:

- the revocation table: a keyed hash of each revoking or gone device's token and when, for 365 days, and the latest time it minted a handle at;
- counters for quotas, in memory: keyed hashes of tokens, slots, and caller addresses as they are (IPv6 cut to its prefix), each forgotten once its allowance has refilled;
- aggregate counts by route and outcome, pushes and proofs.

It writes no request log, and nothing it writes to disk or to its own log names a token, a handle, a caller or a server.

## Operating the relay

`koan-push-relay` reads a TOML file named by `KOAN_RELAY_CONFIG`, overridden by environment variables named `KOAN_RELAY__` and the key path with `__` between levels (`KOAN_RELAY__APNS__PRODUCTION__KEY`). The double underscore after the prefix keeps variables Kubernetes injects for a Service named `koan-relay` out of the configuration; turning off `enableServiceLinks` for the relay's pod avoids them altogether. It refuses to start without a token key, a current handle key and at least one APNs key that can sign, or with a limit of zero.

```toml
listen = "0.0.0.0:8080"
# Prometheus metrics and /healthz, for the cluster only.
metrics_listen = "0.0.0.0:9090"
# Only behind a proxy that sets this header itself. Behind Cloudflare,
# cf-connecting-ip holds the caller alone.
client_ip_header = "cf-connecting-ip"
# Which address in that header, counting back from the end, is the caller:
# 1 for a header holding only the caller, or one proxy appending what it saw
# to x-forwarded-for; 2 behind two proxies that each append.
client_ip_hops = 1
store = "/data/relay.db"
topic = "cc.blit.koan"
# 32 random bytes, base64. Never rotated: revocations are found by it.
token_key = "…"

[handles]
current = 1
# At most 365.
lifetime_days = 60
# 32 random bytes each, base64. Add a key and make it current to rotate;
# remove the old one once lifetime_days have passed.
keys = { "1" = "…" }

[apns]
team_id = "…"
# HTTP/2 connections kept to each gateway, 1 to 64.
connections = 4
# Pushes waiting on Apple at once; beyond it, 503.
max_in_flight = 4096

# Topic-specific keys, scoped to cc.blit.koan, one per gateway.
[apns.production]
key_id = "…"
key_path = "/secrets/production.p8"

[apns.sandbox]
key_id = "…"
key_path = "/secrets/sandbox.p8"

[limits]
handle_alerts_per_hour = 6
handle_wakes_per_hour = 12
device_alerts_per_hour = 20
device_wakes_per_hour = 30
device_revocations_per_hour = 10
ip_pushes_per_minute = 6000
ip_registrations_per_minute = 120
ip_revocations_per_minute = 60
ip_bad_tokens_per_hour = 10
bad_tokens_per_hour = 3600
proofs_per_second = 20
global_pushes_per_second = 20000
# Callers each limiter keeps a count for; past it, a caller it has no count
# for is refused with 503 until idle counts are swept.
max_tracked = 1000000
# Devices the revocation table holds; past it, new revocations get 503.
max_revocations = 1000000
# Requests being worked on at once; past it, 503.
max_requests = 50000
```

Every push is answered from memory, so Apple's round trip, carried over `connections` HTTP/2 connections per gateway, is what bounds throughput.

`/metrics` on the metrics listener counts requests by route and outcome, pushes sent by kind, proofs by Apple's answer, and shows each gateway's breaker (`koan_relay_production_held`, `koan_relay_sandbox_held`), which is worth an alert.

The revocation table is a SQLite file, so one process writes it. Replicas each keep their own quotas, which multiplies the allowances by the number of replicas, and would need a shared revocation table; until there is a need for more than one process, there is one.
