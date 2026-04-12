# Authentication

koan uses Ed25519 JWT tokens for API authentication. Auth is enabled by default.

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
  -d '{"query": "{ libraryStats { trackCount } }"}'
```

### Full curl workflow (copy-pasteable)

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
  -d '{"query": "{ libraryStats { trackCount artistCount albumCount } }"}' | jq

# When access token expires, refresh it (returns new pair)
RESPONSE=$(curl -s -X POST http://localhost:4000/auth/refresh \
  -H "Content-Type: application/json" \
  -d "{\"refresh_token\": \"$REFRESH_TOKEN\"}")

ACCESS_TOKEN=$(echo "$RESPONSE" | jq -r '.access_token')
REFRESH_TOKEN=$(echo "$RESPONSE" | jq -r '.refresh_token')

echo "New access token: ${ACCESS_TOKEN:0:20}..."
```

## CLI authentication

```bash
# Login to a running koan server (stores refresh token in system keyring)
koan auth login http://localhost:4000
# Prompts for username and password interactively

# Token auto-refreshes — no need to login again until the refresh token expires (30 days)
```

## User management

```bash
# List users
koan auth list-users

# Create a user with a specific role (prompts for password interactively)
koan auth create-user --username alice --role user

# Roles:
#   admin    — everything: user management, config, organize, device switching
#   user     — playback, queue, search, favourites, lyrics
#   readonly — browse library, view queue. No mutations.

# Delete a user
koan auth delete-user alice
```

## Token lifecycle

- **Access token**: 15 minutes. Sent as `Authorization: Bearer <token>`.
- **Refresh token**: 30 days. Single-use rotation (each refresh returns a new pair). Stored server-side for revocation.
- **Refresh**: `POST /auth/refresh` with `{"refresh_token": "..."}` returns new access + refresh tokens.
- **Logout**: `POST /auth/logout` with `{"refresh_token": "..."}` revokes the token.

## GraphQL Playground

The playground needs auth too. To avoid the hassle of manually setting headers, koan can generate a pre-authenticated playground URL:

```bash
# Start the playground with a one-time access key in the URL
koan serve --playground
# Opens: http://localhost:4000/playground?key=<one-time-key>
# The key is valid for the session only and printed to stdout
```

The one-time key is injected as a query parameter and translated to a Bearer token server-side. No need to manually set Authorization headers in the playground UI.

If you're running with `auth_enabled = false`, the playground works without any key.

## Keypair

Ed25519 keypair is auto-generated on first `koan auth setup` and stored at:
```
~/.config/koan/auth/ed25519.pem       # private key
~/.config/koan/auth/ed25519_pub.pem   # public key
```

## Recovery / lockout

**Forgot password:**
```bash
# Use the CLI directly (doesn't need a running server or auth)
koan auth create-user --username admin --role admin
# Prompts for new password. Resets password if username already exists.
```

**Regenerate keypair:**
```bash
# Delete the old keypair — all existing tokens will be invalidated
rm ~/.config/koan/auth/ed25519*.pem
koan auth setup
# All users keep their passwords, but must re-login (old JWTs are unsigned by the new key)
```

**Disable auth entirely:**
```toml
# config.toml
[graphql]
auth_enabled = false
```

**Nuclear option (start fresh):**
```bash
# Delete auth state entirely
rm -rf ~/.config/koan/auth/
# Users are in the DB — delete them too if you want a clean slate:
sqlite3 ~/.config/koan/koan.db "DELETE FROM users; DELETE FROM refresh_tokens;"
koan auth setup
```

## Configuration

```toml
# config.toml
[graphql]
auth_enabled = true         # default: true
access_token_ttl = "15m"    # access token lifetime
refresh_token_ttl = "30d"   # refresh token lifetime
```

## In-process access

The TUI and MCP server run in the same process as the player. They bypass auth entirely (injected as anonymous admin). Auth only applies to HTTP API clients (GraphQL, Subsonic REST, web UI).

## Subsonic API

Subsonic REST endpoints use the standard Subsonic auth mechanism (username + token/salt or plain password) for compatibility with existing clients (play:Sub, DSub, Symfonium, etc.). Under the hood, credentials are verified against the same `users` table — same users, same passwords, same roles. The auth mechanism is different (Subsonic's MD5+salt scheme vs JWT) but the identity system is shared.
