# Authentication

kōan uses Ed25519 JWT tokens for API authentication. Auth is enabled by default.

## Quick start

```bash
koan auth setup               # signing keys and the first admin account
koan --headless --port 4000

# Sign in: returns an access token, a refresh token and its lifetime
curl -s -X POST http://localhost:4000/auth/login \
  -H "Content-Type: application/json" \
  -d '{"username": "admin", "password": "your-password"}'

# Call the API with the access token
curl -s http://localhost:4000/graphql \
  -H "Authorization: Bearer $ACCESS_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"query": "{ libraryStats { totalTracks } }"}'

# When it expires, exchange the refresh token for a new pair
curl -s -X POST http://localhost:4000/auth/refresh \
  -H "Content-Type: application/json" \
  -d "{\"refresh_token\": \"$REFRESH_TOKEN\"}"
```

### Non-interactive setup (scripting/CI)

```bash
KOAN_USERNAME=admin KOAN_PASSWORD=secret koan auth setup
KOAN_PASSWORD=secret koan auth create-user --username alice --role user
koan auth delete-user alice --yes
```

`koan auth` never prompts when stdin is not a terminal, or with `--non-interactive`. Credentials come from `KOAN_USERNAME` and `KOAN_PASSWORD`, and a command missing one fails and names it rather than generating a password nobody sees. Deleting a user, regenerating the keypair and `koan auth reset` need `--yes`. Nothing is saved to 1Password unless `--save-to-1password` is passed, since an unanswered prompt is not consent to write to someone's vault.

## CLI authentication

```bash
# Sign in to a kōan server; asks for the password (or reads KOAN_PASSWORD) and keeps the refresh token in config.local.toml
koan auth login --server http://localhost:4000 --username admin
```

`koan play --server <url>` signs in with the stored token when its `server`
matches `<url>`. Access tokens are short-lived; when the server refuses one,
koan spends the refresh token for a new pair and retries once. The server
revokes a refresh token on use, so each refresh writes its replacement back to
`config.local.toml`. Each replacement starts a fresh `refresh_token_ttl` (30
days by default), so a machine that connects that often stays signed in until
`koan auth logout`. A refused or missing sign-in stops
`koan play --server` at startup with the server's reason.

## User management

Admins manage accounts on the web UI's Users page, from the apps' Settings, over
GraphQL (`users`, `createUser`, `inviteUser`, `setUserRole`, `setUserPassword`,
`deleteUser`, which MCP clients can call too), or with the CLI.

The server keeps only an argon2 hash of each password, never a copy it can read
back, so no admin can see an account's password. A generated one is shown once,
when it is made.

### Signing in with a password

kōan's apps and terminal UI sign in to a kōan server with a username and
password once. The server lists the `koanSignIn` extension; seeing it, the
client sends the password as `p=enc:` to `/rest/koanSignIn`, over plain HTTP
too, and gets back an API key of the device's own, named after it. It keeps the
key in `config.local.toml` and not the password, which is never sent again.
Signing in again from the same device replaces that device's key, so a
reinstall leaves no unused key behind. Only the account's own password is
traded for a key: an app password or the shared secret is refused there (error
50), and the client then keeps it as typed and signs with it as a salted token,
as before. Against any other Subsonic server the password
is kept as before, and sent as a salted token over plain HTTP. A server that
refuses token sign-in (error 41) is reported as needing an app password or API
key.

### Invites

An invite is a link carrying the server, the username and a token:
`https://koan.rocks/join/#server=…&username=…&invite=…`. On a device with koan
installed it opens the app, which trades the token for an API key of its own
(`/rest/koanJoin`, listed as the `koanInvite` extension), signs in with that key
and syncs the library with no further steps. Each device the link is opened on
gets its own key, named after the device, which appears under API keys and can
be revoked on its own. Elsewhere, koan.rocks shows the downloads and an Open in
koan button. The Mac app is not a universal link target, since a Developer ID
build carries no associated domains: there the page opens and its button hands
over through `koan://join`. Pasting the link into Server URL in Settings works
on either. The link is in the fragment, which browsers never send, so koan.rocks
does not see it.

The token is a JWT signed with the server's key, naming the account and good for
a week. Nothing is stored when one is made, so an admin can make another at any
time. It also carries a short digest of the account's password hash, so any
password change withdraws every link sent before it: a reset is how a link that
went to the wrong place is taken back. Links from servers older than tokens,
which carried the password, are no longer read.

Opening a link again on a device that already joined replaces that device's key
and revokes the old one, through `/rest/koanRevokeKey`, which revokes the key
the request signs in with.

Creating an account generates its password, and the email carries it once, for
the web UI and other Subsonic apps. Inviting an existing account sends only the
link. To give an account a new password, invite it with a reset (generated,
shown in the invite) or set one on the Users page or with `setUserPassword`.
Either signs every device out, invited ones included, since a password change
revokes the account's sessions and API keys.

A device whose key was revoked, or whose password no longer works, finds out
the next time it syncs or asks the server what it offers (Subsonic errors 40,
41 or 44). kōan's apps then say so in Settings → Server and on their empty
pages, rather than waiting for a sync that cannot succeed, until the device
signs in again.

### Pairing a device

A device without a keyboard, such as the Apple TV app, cannot reasonably take a
password or a pasted invite, so it signs in by being approved from somewhere
that is already signed in. It opens a WebSocket at `/rest/koanPair` (listed as
the `koanPair` extension) with no credentials and is given a code, shown as
`XXXX-XXXX`, and a link, `https://koan.rocks/pair/#s=…&p=…`. Opening the link in
koan on a phone or Mac, typing the code under Settings → Server → Pair a device,
or typing it on the server's `/pair` page asks "Sign in this device?"; approving
makes an API key on the approver's account, named after the device, and the
server sends it down the waiting socket. The device is told the moment it is
approved or declined; nothing polls. A pairing lasts ten minutes
(`KOAN_PAIR_TTL_SECS` in the server's environment changes that, for trying
expiry out) and lives only in the server's memory. Approving signs the device in as you, so approve only a
device you are setting up yourself: the name it shows is whatever it chose to
call itself. Every approval screen also says where the request came from: the
address the server saw (behind a trusted proxy, the client's, as the rate limits
use it), and whether that is on a private network or the internet. Private
means RFC 1918, shared (100.64.0.0/10, which Tailscale uses), link-local,
unique local or loopback. The classification is the server's view: with the
server on the same network, a television in the same room asks from a private
address and a request from the internet is worth declining unless you expected
it; with the server on the internet, every device at home asks from your public
address.

A reverse proxy or tunnel in front of the server must send `X-Forwarded-For`.
Without it the server sees the proxy's address, which is usually loopback or
private, so every request, from wherever, is shown as on your network.

`/rest/koanPair` refuses any request carrying an `Origin` header, which every
browser sends on a WebSocket and the apps do not: otherwise a web page someone
on your network visits could open a pairing from their address and read the key
sent when they approve it. One address, or one IPv6 /64, can have three
pairings waiting at a time.

The server sends no mail. Creating an account or inviting one produces the email
(plain text, rich text with a button, and a `mailto:`) for the admin to send
themselves.

The link points at `sharing.public_url` when it is set, and otherwise at the
address the admin reached the server on (honouring `X-Forwarded-Host` and
`X-Forwarded-Proto` from a proxy).

```bash
# List users
koan auth list-users

# Create a user (offers to generate a secure password, offers to save to 1Password)
koan auth create-user --username alice --role user

# Roles:
#   admin    — everything: user management, config, organize, device switching
#   user     — playback, queue, search, favourites, lyrics
#   readonly — browse library, view queue. No mutations.

# Reset a user's password (revokes their refresh tokens and API keys)
koan auth reset-password alice

# Change a user's role
koan auth set-role alice admin

# Print an invite: the link and the email to send it in
# (--reset-password also generates a new password and puts it in the email)
koan auth invite alice --server https://music.example.com

# Delete a user
koan auth delete-user alice
```

## Sharing devices between accounts

An account's devices are its own. To let someone with another account on the
server control one, share it from the device: see
[What another device may do](devices.md#what-another-device-may-do).
The grant names the device, its owner and the other account. The server relays
the playback set for it (play, the queue, the output, the preset, the volume,
hand-off) as the other account's request, never with the owner's powers, so
nothing of the owner's library, settings, favourites, playlists or history is
reachable. Any signed-in account sees the server's usernames when sharing.
Deleting either account leaves the grant unused; stopping the share removes
it.

## Token lifecycle

- **Access token**: 15 minutes. Sent as `Authorization: Bearer <token>`.
- **Refresh token**: 30 days. Single-use rotation (each refresh returns a new pair). Stored server-side for revocation.
- **Refresh**: `POST /auth/refresh` with `{"refresh_token": "..."}` returns new access + refresh tokens.
- **Logout**: `POST /auth/logout` with `{"refresh_token": "..."}` revokes the token.

## OAuth, for MCP clients

With `sharing.public_url` set, the server is an OAuth 2.1 authorization server for its own `/mcp`: discovery at `/.well-known/oauth-protected-resource/mcp` and `/.well-known/oauth-authorization-server`, open registration at `/oauth/register`, consent at `/oauth/authorize` behind the web UI's sign-in, and `/oauth/token` (authorization code with PKCE S256, and refresh). What it issues are kōan access and refresh tokens scoped to MCP (`"scope": "mcp"` in the JWT): `/mcp` accepts only those, and GraphQL and the web UI refuse them, so a connection cannot do more through GraphQL than MCP allows it. The account is looked up on every request, as for any session.

Two things differ from an app's session. A client id is a JWT signed with the server's key, carrying the client's redirect URIs, so registrations need no storage and rotating the keys sends every client back to register. And each connection's refresh tokens are one grant: a spent refresh token presented again more than 30 seconds later, or a code exchanged twice, means a copy is in other hands, and revokes the whole grant. App and browser sessions are exempt, since tabs and tasks sharing one session may race a refresh.

`/oauth/register` allows 10 registrations per IP per hour and `/oauth/token` 60 requests per IP per minute.

## GraphQL Playground

```bash
koan --headless --playground
# Prints: GraphiQL: http://127.0.0.1:4000/graphql?introspection-key=<uuid>
# Auto-opens the browser. The introspection key is injected into all requests.
# Key is process-scoped — dies when the server exits.
```

The playground page only renders with the correct `?introspection-key=` param (403 otherwise). The key is injected as an `X-Introspection-Key` header on every GraphQL request. Normal JWT auth still works for real API clients.

## 1Password integration

With the `op` CLI installed, creating a user or resetting a password at a terminal offers to generate a 32-character password and to save it to 1Password as `koan@hostname`, asking again before updating an existing item. `--save-to-1password` saves without asking, and is the only way it happens without a terminal.

## Keypair

Ed25519 keypair is auto-generated on first `koan auth setup` and stored at:
```
~/.config/koan/auth/ed25519.pem       # private key (0600 perms)
~/.config/koan/auth/ed25519_pub.pem   # public key
~/.config/koan/auth/.gitignore        # wildcard * — prevents commits
```

## Recovery / lockout

**Reset a password:**
```bash
koan auth reset-password admin
# Prompts for new password. Revokes that user's refresh tokens and API keys.
```

**Regenerate keypair:**
```bash
koan auth regenerate-keys
# Generates new Ed25519 keypair. All existing tokens are invalidated.
# Users keep their passwords but must re-login.
```

**Disable auth entirely:**
```toml
# config.toml
[graphql]
auth_enabled = false
```

> **Warning:** with auth disabled, anything that can reach the port is an admin — it can read your
> entire library, control playback and rewrite config. The `Origin` and `Host` checks keep a web page
> you visit from being that "anything", but they are not a substitute for auth: any other machine on
> the network still gets in. Only disable auth on a host you control, bound to `127.0.0.1`, and never
> with the port forwarded.

**Start fresh:**
```bash
koan auth reset
# Deletes all keys, users, and tokens. Prompts for confirmation.
# Then: koan auth setup
```

## Configuration

```toml
# config.toml
[graphql]
enabled = true            # default: true — API starts with TUI
auth_enabled = true       # default: true
access_token_ttl = "15m"  # access token lifetime
refresh_token_ttl = "30d" # refresh token lifetime
```

## In-process access

The TUI runs in the same process as the player and bypasses auth entirely. `koan mcp` does too, but executes at `user` role rather than admin — its transport carries no credential, so anything it can reach is reachable by whoever can talk to the MCP process. A server's `/mcp` acts as the signed-in account, with an admin account capped the same way. That leaves out the mutations that move files (`organize*`), rewrite config, trigger scans, or change the output device. Set `KOAN_MCP_ADMIN=1` to opt back in.

Auth otherwise applies to HTTP API clients (GraphQL, web UI). The web UI can also take the account from an authenticating reverse proxy; see [Behind an authenticating proxy](headless-server.md#behind-an-authenticating-proxy). The Subsonic REST API is separate — see below.

## Cookies

`/auth/login` sets two `HttpOnly` cookies: `koan_access` (the JWT, `Path=/`) and `koan_refresh` (`Path=/auth/refresh`, so it is never attached to an API call). Both are `SameSite=Lax`, which is what keeps them off cross-site requests — a foreign page cannot use them to open a WebSocket or fire a mutation.

The refresh token is also returned in the login response body, because the CLI and other non-browser clients have no cookie jar and keep it in `config.local.toml`.

Refresh tokens are stored in the database as `sha256(token)`, so a database read yields nothing usable.

## Sockets

A socket is authenticated once, when it opens, so `/graphql/ws` and an app's link at `/rest/koanLink` close whenever something about their account changes that can narrow what it may do: its role, its password, its deletion, a key or app password revoked, or a device's key replaced when it signs in again. Signing out does not: no socket rests on a refresh token, so the account's other devices stay connected. A subscription socket also closes when the token it opened with expires. The client reconnects and is authenticated as things then stand. A change made by another process, such as `koan auth` at a terminal while the server runs, reaches sockets when they next reconnect.

## Subsonic API

`/rest/*` is kōan's Subsonic REST API, with the OpenSubsonic extensions `apiKeyAuthentication`, `formPost` and `songLyrics` (listed, without sign-in, by `getOpenSubsonicExtensions`), and koan's own. Clients sign in with one of:

- **API key** (`apiKey=`) — preferred. A key acts as the account that made it, at that account's current role, until revoked; it is sent without `u`, and sending it with `u` or any other credential is error 43. Keys are 32 random bytes and only `sha256(key)` is stored, so a key is shown once, when it is made.
- **Account password** (`p=`, plain or `enc:` hex) — checked against the account's argon2 hash; a successful check is remembered for ten minutes. argon2 is expensive by design, so at most one check per core (2 to 8) runs at once and a request arriving when all are busy gets error 0, "server busy", rather than waiting. The protocol sends the password with every request, so use it only over HTTPS.
- **App password** (`u` + `t` + `s`, or `p=`) — for clients that only sign in with Subsonic token auth. Made per app on the web UI's Account page, shown once, and usable until revoked; changing the account's password revokes them all.
- **Shared secret** (`u` + `t` + `s`, or `p=`) — the optional `[subsonic]` secret and its username, acting as `user`, for clients that have no account.

Which credential a client should use:

| Client | Credential |
| --- | --- |
| The web UI | The account's password |
| kōan's apps, and Subsonic clients that support OpenSubsonic API keys | An API key (kōan's apps get one from an invite, pairing, or by signing in with the password once) |
| Subsonic clients that sign in with a token (`t`/`s`) | An app password |
| Older clients that send the password itself (`p=`) | The account's password over HTTPS, or an app password |

### Sign-in limits

Every request carries its credential, so failed password sign-ins are limited three ways: per address and username (10 a minute), per address (30), and per username from every address together (60). The last is shared with `/auth/login` and the web UI's sign-in, so spreading guesses across addresses or doors gains nothing. Since anyone can spend that budget, it does not apply to a network the account signed in from in the last week, by password or by a browser refreshing its session: an outsider cannot lock the account's own people out. Only failures count, so a client syncing a library is never slowed. API keys and the shared secret's token are random and not worth guessing, so they are never limited, and a flood of wrong passwords for an account cannot lock out the apps signed in with either. An IPv6 address counts as its /64.

Subsonic token auth (`t = md5(password + salt)`) cannot be checked against the account's own password, because the server keeps only its argon2 hash; a password the server can read back is one its admins and anyone with the database can read too. App passwords are the exception made for token-only clients. Each is random, never the account's password, and stored sealed with ChaCha20-Poly1305 under a key derived (HKDF) from the server's Ed25519 signing key and bound to its account, so a copy of the database alone does not yield them, and regenerating the keypair retires them with every session. An account without app passwords gets error 41 for a token, which tells a client to fall back to a password or a key. Token auth still protects little in transit — a captured token replays — so use HTTPS.

```bash
koan auth api-key create --username alice --name phone   # prints the key once
koan auth api-key list [--username alice]
koan auth api-key revoke 3

koan subsonic setup           # generate the shared secret, enable /rest/*
koan subsonic status
koan subsonic disable
```

Signed-in users manage their own keys in the web UI under **API keys**.

A koan app that joined by invite keeps its key as `remote.api_key` in `config.local.toml`, and signs in with it ahead of any password, `KOAN_REMOTE__PASSWORD` included. Sign out to go back to a password. `readonly` accounts, and their keys, get error 50 from every endpoint that writes.

