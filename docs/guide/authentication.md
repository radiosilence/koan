# Authentication

kōan uses Ed25519 JWT tokens for API authentication. Auth is enabled by default.

## Quick start

```bash
# 1. Set up auth (generates Ed25519 keypair + creates admin user)
koan auth setup

# 2. Start the server
koan --headless --port 4000  # or: koan play (starts API alongside TUI)

# 3. Get a token
curl -s -X POST http://localhost:4000/auth/login \
  -H "Content-Type: application/json" \
  -d '{"username": "admin", "password": "your-password"}'

# Response:
# { "access_token": "eyJ...", "refresh_token": "...", "expires_in": 900 }

# 4. Use the token
curl -s http://localhost:4000/graphql \
  -H "Authorization: Bearer eyJ..." \
  -H "Content-Type: application/json" \
  -d '{"query": "{ libraryStats { totalTracks totalArtists totalAlbums } }"}' | jq
```

### Full curl workflow

```bash
# Login and capture tokens
RESPONSE=$(curl -s -X POST http://localhost:4000/auth/login \
  -H "Content-Type: application/json" \
  -d '{"username": "admin", "password": "your-password"}')

ACCESS_TOKEN=$(echo "$RESPONSE" | jq -r '.access_token')
REFRESH_TOKEN=$(echo "$RESPONSE" | jq -r '.refresh_token')

echo "Access token (15min):  ${ACCESS_TOKEN:0:20}..."
echo "Refresh token (30d):   ${REFRESH_TOKEN:0:20}..."

# Make authenticated GraphQL requests
curl -s http://localhost:4000/graphql \
  -H "Authorization: Bearer $ACCESS_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"query": "{ libraryStats { totalTracks totalArtists totalAlbums } }"}' | jq

# When access token expires, refresh it (returns new pair)
RESPONSE=$(curl -s -X POST http://localhost:4000/auth/refresh \
  -H "Content-Type: application/json" \
  -d "{\"refresh_token\": \"$REFRESH_TOKEN\"}")

ACCESS_TOKEN=$(echo "$RESPONSE" | jq -r '.access_token')
REFRESH_TOKEN=$(echo "$RESPONSE" | jq -r '.refresh_token')

echo "New access token: ${ACCESS_TOKEN:0:20}..."
```

### Non-interactive setup (scripting/CI)

```bash
# Use environment variables to skip interactive prompts
KOAN_USERNAME=admin KOAN_PASSWORD=secret koan auth setup
KOAN_PASSWORD=secret koan auth create-user --username alice --role user
```

## CLI authentication

```bash
# Login to a running koan server (stores the refresh token in config.local.toml)
koan auth login --server http://localhost:4000 --username admin
# Prompts for password interactively

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
time; a token cannot be withdrawn before it expires, short of deleting the
account or rotating the server's keys (`koan auth regenerate-keys`, which also
signs everyone out). Links from servers older than tokens carry the password
instead, and the apps still sign in with them.

Creating an account generates its password, and the email carries it once, for
the web UI and other Subsonic apps. Inviting an existing account sends only the
link. To give an account a new password, invite it with a reset (generated,
shown in the invite) or set one on the Users page or with `setUserPassword`.
Either signs every device out, invited ones included, since a password change
revokes the account's sessions and API keys.

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

If the `op` CLI is detected on your system:

- **Password generation**: offered on user creation (`[Y/n]` — generates a 32-char random password and prints it)
- **Credential saving**: offered after creation (`Save to 1Password as 'koan@hostname'? [Y/n]`)
- **Updates**: if a `koan@hostname` item already exists, offers to update it instead of creating a duplicate

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

Auth otherwise applies to HTTP API clients (GraphQL, web UI). The Subsonic REST API is separate — see below.

## Cookies

`/auth/login` sets two `HttpOnly` cookies: `koan_access` (the JWT, `Path=/`) and `koan_refresh` (`Path=/auth/refresh`, so it is never attached to an API call). Both are `SameSite=Lax`, which is what keeps them off cross-site requests — a foreign page cannot use them to open a WebSocket or fire a mutation.

The refresh token is also returned in the login response body, because the CLI and other non-browser clients have no cookie jar and keep it in `config.local.toml`.

Refresh tokens are stored in the database as `sha256(token)`, so a database read yields nothing usable.

## Subsonic API

`/rest/*` is kōan's Subsonic REST API, with the OpenSubsonic extensions `apiKeyAuthentication`, `formPost` and `songLyrics` (listed, without sign-in, by `getOpenSubsonicExtensions`), and koan's own. Clients sign in one of three ways:

- **API key** (`apiKey=`) — preferred. A key acts as the account that made it, at that account's current role, until revoked; it is sent without `u`, and sending it with `u` or any other credential is error 43. Keys are 32 random bytes and only `sha256(key)` is stored, so a key is shown once, when it is made.
- **Account password** (`p=`, plain or `enc:` hex) — checked against the account's argon2 hash; a successful check is remembered for ten minutes. argon2 is expensive by design, so at most one check per core (2 to 8) runs at once and a request arriving when all are busy gets error 0, "server busy", rather than waiting. The protocol sends the password with every request, so use it only over HTTPS.
- **Shared secret** (`u` + `t` + `s`, or `p=`) — the optional `[subsonic]` secret, for clients that only speak token auth.

Subsonic token auth (`t = md5(password + salt)`) is refused for accounts with error 41, which tells a client to fall back to a password or a key. Checking it needs the plaintext password on the server, and a password the server can read back is one its admins can read too. It also protects little: a captured token replays, and md5 of a short password is cheap to reverse.

```bash
koan auth api-key create --username alice --name phone   # prints the key once
koan auth api-key list [--username alice]
koan auth api-key revoke 3

koan subsonic setup           # generate the shared secret, enable /rest/*
koan subsonic status
koan subsonic disable
```

Signed-in users manage their own keys in the web UI under **API keys**. `readonly` accounts, and their keys, get error 50 from every endpoint that writes.

`koan play --server` streams audio over `/rest/stream` and signs those requests with the `[subsonic]` credentials from the machine it runs on, so they have to match the server's.
