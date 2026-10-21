# Push relay

Apple delivers pushes to the kōan iOS app only when they are signed with a key belonging to the developer's team, so a self-hosted kōan server cannot wake a suspended iPhone or ask it to play on its own. The push relay at `push.koan.rocks` holds a topic-specific key of its own and forwards pushes for any server. It is built so that the worst a hostile server can do through it is make a phone show kōan's own generic text, within quotas, to people who signed in to that server.

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

A handle opens for 60 days from minting (`expires` in the registration response). The app registers again before then, at half its lifetime, and gives its servers the new handle on their next link.

The app registers once for each server and gives every server a handle of its own, never one handle to several. Each handle has its own quota, so a server that spends its allowance spends no other server's; the device's ceiling bounds them all together (see [Quotas](#quotas)).

The relay keeps no record of handles. It can rotate its sealing key: handles sealed under a key it still lists keep opening, and a key is retired once every handle sealed under it has expired.

## Revocation

A device revokes by sending its token. Every handle for that token minted at or before that moment stops opening; handles minted afterwards are unaffected. The app revokes when it signs out of a server, then registers again and gives each server it still uses a fresh handle. Because revoking needs the token, only the device can do it: a server cannot cut a device off from its other servers.

The app waits for the revocation's `204` before registering again, and registers again after any revocation that succeeds, including a retried one: a revocation retires every handle minted before it reached the relay, whichever request asked for them.

Before recording a revocation the relay sends the token a background push, and records it only if Apple accepts it. The table therefore holds only devices that exist, and cannot be filled with made-up tokens. The app is open when it revokes, so the wake asks nothing of it.

The relay stores one row per revoking device: a keyed hash of the token and the time. A row is deleted once every handle it could refuse has expired.

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

`expires` is in Unix seconds. `400` means the token is not 8 to 100 bytes of hex, or the relay holds no key for the requested gateway.

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
| `wake-now` | as `wake`, expires in a minute | the same, for a wake that is no use late |
| `summon` | alert, time-sensitive, collapse id `koan-summon`, expires in two minutes | the relay's notification |
| `notify` | alert, time-sensitive, expires in ten minutes | the relay's notification |

The relay's notification is titled "kōan" and reads "Something is waiting to play. Tap to open." A `summon` or `notify` may carry `blob`, at most 3072 base64url characters, which the relay passes to the app under `koan-e2e` with `mutable-content` set. The app's notification service extension decrypts it and, only if it authenticates, replaces the text and acts on what it carries; otherwise the relay's text is shown and tapping opens the app. A `wake` or `wake-now` with a blob is refused with `400`. The blob's format is the app's and its servers' business; the relay does not read it.

| Status | Meaning | What the server does |
|--------|---------|----------------------|
| `204` | Apple accepted the push | |
| `400` | The request is malformed | Fix the request |
| `410` | The handle does not open (unknown, altered, expired, revoked) or Apple says the token is no longer valid | Forget the handle; the app sends a new one when it next links |
| `429` | A quota is spent, or Apple is throttling the device | Wait at least `Retry-After` seconds |
| `503` | The relay or Apple cannot take the push now | Retry after `Retry-After` seconds |

Live Activity updates are not relayed. ActivityKit renders their content without passing it through the app, so they cannot be encrypted, and relaying them would show the relay what people are listening to.

## Quotas

Each relay process counts:

| Counted per | Default |
|-------------|---------|
| Handle: notifications | 30 an hour |
| Handle: wakes | 60 an hour |
| Device, across its handles: notifications | 120 an hour |
| Device, across its handles: wakes | 240 an hour |
| Caller address: pushes | 6000 a minute |
| Caller address: registrations and revocations | 30 a minute |
| Everyone: pushes sent to Apple | 20,000 a second |

A device is counted by its token, so its ceiling holds whichever servers hold its handles. A caller address is counted by its IPv4 address or its IPv6 /64. Each count allows its whole allowance as a burst and then refills evenly over the period. A push refused because the relay or Apple could not take it is not counted against its handle or device. Behind a proxy, the address is the last one in the last instance of the header the relay is configured to trust: the one the nearest proxy wrote.

## What the relay sees and keeps

It sees, per request: a device token (registration and revocation only), a handle, which it opens to a token in memory, the kind of push, an encrypted blob it cannot read, and the caller's address.

It keeps: the revocation table (a keyed hash of each revoking device's token and when it revoked, for at most 60 days), counters for quotas in memory (keyed hashes of tokens, handles' random nonces, and caller addresses as they are, IPv6 cut to its /64; each forgotten once its allowance has refilled), and aggregate request counts by route and outcome. It writes no request log, and nothing it writes to disk names a token, a handle, a caller or a server.

## Operating the relay

`koan-push-relay` reads a TOML file named by `KOAN_RELAY_CONFIG`, overridden by `KOAN_RELAY_*` environment variables with `__` between levels (`KOAN_RELAY_APNS__PRODUCTION__KEY`). It refuses to start without a token key, a current handle key and at least one APNs key.

```toml
listen = "0.0.0.0:8080"
# Prometheus metrics and /healthz, for the cluster only.
metrics_listen = "0.0.0.0:9090"
# Only behind a proxy that sets this header itself.
client_ip_header = "x-forwarded-for"
store = "/data/relay.db"
topic = "cc.blit.koan"
# 32 random bytes, base64. Never rotated: revocations are found by it.
token_key = "…"

[handles]
current = 1
lifetime_days = 60
# 32 random bytes each, base64. Add a key and make it current to rotate;
# remove the old one 60 days later.
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
handle_alerts_per_hour = 30
handle_wakes_per_hour = 60
device_alerts_per_hour = 120
device_wakes_per_hour = 240
ip_pushes_per_minute = 6000
ip_registrations_per_minute = 30
global_pushes_per_second = 20000
```

Every push is answered from memory, so Apple's round trip, carried over `connections` HTTP/2 connections per gateway, is what bounds throughput. Pushes waiting on Apple beyond `max_in_flight`, and requests beyond `limits.max_requests`, are refused with `503` rather than queued.

The revocation table is a SQLite file, so one process writes it. Replicas each keep their own quotas, which multiplies the allowances by the number of replicas, and would need a shared revocation table; until there is a need for more than one process, there is one.
