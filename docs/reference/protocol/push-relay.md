# Push relay

Apple delivers pushes to the kōan iOS app only when they are signed with a key belonging to the developer's team, so a self-hosted kōan server cannot wake a suspended iPhone or ask it to play on its own. The push relay at `push.koan.rocks` holds a topic-specific key of its own and forwards pushes for any server. The key belongs to the developer's Apple account, which answers to Apple for every push sent under it, so the relay is built to bound what anyone can cause through it: the worst a hostile server can do is make a phone show kōan's own text, within small quotas, to people who chose to sign in to that server, and nobody can make the relay send Apple a flood, a stream of errors, or anything a server wrote.

That bound holds only together with the app: the notification service extension shows nothing a server wrote, only its own template filled with structured fields from the blob (see [What a phone shows](#what-a-phone-shows)).

The relay is `crates/koan-push-relay`. This page specifies its API; running it is under [Operating the relay](#operating-the-relay).

## Parties

- **The app** holds its APNs device token. It registers the token with the relay and receives a handle, which it gives its servers. Only the app and Apple ever hold the token.
- **A server** holds handles its users' apps gave it, and asks the relay to push to them. It never sees a token.
- **The relay** opens handles, applies quotas and writes everything Apple is sent, apart from an encrypted blob the server may attach to a notification.

## Handles

A handle is the device token, the gateway (production or sandbox) and the time it was minted, sealed with AES-256-GCM-SIV under a key only the relay holds, and encoded as base64url without padding:

```text
version (1) | key id (1) | nonce (12) | sealed( flags (1) | minted ms (8, big-endian) | token ) | tag (16)
```

The version and key id are authenticated with the rest. A handle is opaque to everyone but the relay: clients store it and send it back unchanged, and must not parse it. It is at most 188 characters.

A handle opens for 60 days from minting by default, and at most 365 (`expires` in the registration response). The app registers again before then, at half its lifetime, and gives its servers the new handle on their next link.

The app registers once for each server and gives every server a handle of its own, never one handle to several. Each handle has its own quota, so a server that spends its allowance spends no other server's; the device's ceiling bounds them all together (see [Quotas](#quotas)).

The relay keeps no record of handles. It can rotate its sealing key: handles sealed under a key it still lists keep opening, and a key is retired once every handle sealed under it has expired.

## Revocation

A device revokes by sending its token. Every handle for that token minted at or before that moment stops opening, and so does any minted in the ten minutes after it, so that a clock stepped back by up to that much cannot bring a revoked handle back; a registration after a revocation is minted past that window. The app revokes when it signs out of a server, then registers again and gives each server it still uses a fresh handle. Because revoking needs the token, only a holder of the token can do it, and the app gives its token only to the relay and to servers that push with the developer's own key; any other server holds handles alone and cannot cut a device off from its other servers.

The app waits for the revocation's `204` before registering again, and registers again after any revocation that succeeds, including a retried one: a revocation retires every handle minted before it reached the relay, whichever request asked for them.

The relay stores one row per revoking device: a keyed hash of the token and the time. A row is kept for 365 days, the longest a handle can be configured to open, so changing the configured lifetime never lets a revoked handle open again.

When Apple answers a push that the token is no longer valid, the relay records a revocation for that device itself, so a server that ignores the `410` cannot keep sending to it.

## Proof of a device

Before minting a handle or recording a revocation, the relay sends the token a background push (`wake-now`) and goes on only if Apple accepts it. Handles and revocations therefore exist only for real devices: neither can be had for a made-up token, so the revocation table cannot be filled and a relay's quotas cannot be multiplied with invented devices. The app is open when it registers or revokes, so the wake asks nothing of it.

Proofs are the one place a made-up token reaches Apple, so they are limited apart from pushes. They have a global rate of their own. The relay counts the tokens Apple refuses as unknown, and once that count passes its hourly limit it sends no proof at all, answering `503`, until the count falls back; registrations wait meanwhile. A proof counts against the device's allowance only once Apple has accepted the token, so a made-up token is never tracked: a registration's proof against the device's wakes, a revocation's against a separate per-device allowance, so a server spending a device's wakes cannot stop it revoking.

## Endpoints

All requests are `POST` with a JSON body of at most 8 KiB. Unknown fields are refused with `400`, unlike the link protocol: the relay accepts nothing it does not write itself. Errors have the body `{"error": "<reason>"}`; the reason is for logs, and clients act on the status alone.

### `POST /v1/register`

Called by the app.

```json
{ "token": "<device token, hex>", "sandbox": false }
```

`sandbox` is true for development builds, whose tokens only Apple's sandbox gateway accepts; it may be absent. The answer is `200`:

```json
{ "handle": "<handle>", "expires": 1767225600 }
```

`expires` is in Unix seconds. `400` means the token is not 8 to 100 bytes of hex, the relay holds no key for the requested gateway, or Apple refused the token; `429` and `503` are as for a push.

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
| `summon` | alert, collapse id `koan-summon`, expires in two minutes | the relay's notification |
| `notify` | alert, collapse id `koan-play`, expires in ten minutes | the relay's notification |

The relay's notification is titled "kōan", reads "Something is waiting to play. Tap to open.", plays the default sound and has the interruption level `active`: never `time-sensitive`, since the relay cannot vouch for the server, so it does not break through a Focus. A `summon` or `notify` may carry `blob`, at most 3072 base64url characters, which the relay passes to the app under `koan-e2e` with `mutable-content` set. A `wake` or `wake-now` with a blob is refused with `400`. The relay does not read the blob.

| Status | Meaning | What the server does |
|--------|---------|----------------------|
| `204` | Apple accepted the push | |
| `400` | The request is malformed | Fix the request |
| `410` | The handle does not open (unknown, altered, expired, revoked), the relay no longer serves its gateway, or Apple says the token is no longer valid | Forget the handle; the app sends a new one when it next links |
| `429` | A quota is spent, or Apple is throttling the device | Wait at least `Retry-After` seconds |
| `503` | The relay or Apple cannot take the push now, or Apple is refusing the relay itself (`Retry-After` is then minutes) | Retry after `Retry-After` seconds |

### What a phone shows

The extension decrypts the blob, but decrypting proves only that the sender holds the device's public key, which every server the person signed in to does. So the extension shows nothing a server wrote:

- The blob carries structured fields, never display text: the command, and the title and artist of what it would play.
- The extension fills its own template with them, naming the server as the person saved it. Fields are capped in length and stripped of links, phone numbers, addresses and control characters. A title or body in the blob is ignored.
- Cover art is fetched only from the signed-in server's own origin, must be an image, and is capped in size.
- Tapping runs only playback commands.
- When the blob does not decrypt or does not fit, the relay's text stands and tapping opens the app.

Live Activity updates are not relayed. ActivityKit renders their content without passing it through the app, so they cannot be encrypted, and relaying them would show the relay what people are listening to.

## Quotas

Each relay process counts:

| Counted per | Default |
|-------------|---------|
| Handle: notifications | 6 an hour |
| Handle: wakes | 12 an hour |
| Device, across its handles: notifications | 20 an hour |
| Device, across its handles: wakes, proofs of registration included | 30 an hour |
| Device: revocations | 10 an hour |
| Caller address: pushes | 6000 a minute |
| Caller address: registrations | 120 a minute |
| Caller address: revocations | 60 a minute |
| Everyone: proofs sent to Apple | 20 a second |
| Everyone: tokens Apple refuses in proofs, before proofs stop | 600 an hour |
| Everyone: pushes sent to Apple | 20,000 a second |

A device is counted by its token, so its ceiling holds whichever servers hold its handles. A caller address is counted by its IPv4 address, or its IPv6 /64 for pushes and /48 for registrations and revocations, the larger block one site is commonly given. Each count allows its whole allowance as a burst and then refills evenly over the period. A push refused because the relay or Apple could not take it is not counted against its handle or device. Behind proxies, the address is read from the header the relay is configured to trust, counting `client_ip_hops` addresses back from the end across all its lines: the proxies append, so anything further back came from the caller. The relay should be reachable only through the proxy that writes the header, since anyone reaching it directly can write it themselves.

## When Apple refuses the relay

When Apple refuses the relay's own credential (an invalid or over-refreshed signing key, a wrong topic or environment), every push would be refused the same way, and sending on with a credential Apple has refused is what gets a key looked at. The relay stops: every push and proof is answered `503` with `Retry-After: 300` without reaching Apple, and one a minute is let through to learn whether it has been fixed. The first success closes it.

## What the relay sees and keeps

It sees, per request: a device token (registration and revocation only), a handle, which it opens to a token in memory, the kind of push, an encrypted blob it cannot read, and the caller's address.

It keeps: the revocation table (a keyed hash of each revoking or gone device's token and when, for 365 days), counters for quotas in memory (keyed hashes of tokens, handles' random nonces, and caller addresses as they are, IPv6 cut to its prefix; each forgotten once its allowance has refilled), and aggregate request counts by route and outcome. It writes no request log, and nothing it writes to disk or to its own log names a token, a handle, a caller or a server.

## Operating the relay

`koan-push-relay` reads a TOML file named by `KOAN_RELAY_CONFIG`, overridden by `KOAN_RELAY_*` environment variables with `__` between levels (`KOAN_RELAY_APNS__PRODUCTION__KEY`). It refuses to start without a token key, a current handle key and at least one APNs key that can sign.

```toml
listen = "0.0.0.0:8080"
# Prometheus metrics and /healthz, for the cluster only.
metrics_listen = "0.0.0.0:9090"
# Only behind a proxy that sets this header itself. Behind Cloudflare,
# cf-connecting-ip holds the caller alone.
client_ip_header = "cf-connecting-ip"
# Addresses the proxies append at the end of the header: 1 for a header
# holding only the caller, or one proxy appending to x-forwarded-for.
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
connections = 4
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
proofs_per_second = 20
bad_tokens_per_hour = 600
global_pushes_per_second = 20000
```

Every push is answered from memory, so Apple's round trip, carried over `connections` HTTP/2 connections per gateway, is what bounds throughput. Pushes waiting on Apple beyond `max_in_flight`, and requests beyond `limits.max_requests`, are refused with `503` rather than queued.

The revocation table is a SQLite file, so one process writes it. Replicas each keep their own quotas, which multiplies the allowances by the number of replicas, and would need a shared revocation table; until there is a need for more than one process, there is one.
