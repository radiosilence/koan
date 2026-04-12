# Authentication

koan uses Ed25519 JWT tokens for API authentication. Auth is enabled by default.

## Quick start

```bash
# 1. Set up auth (generates Ed25519 keypair + creates admin user)
koan auth setup

# 2. Start the server
koan play  # or: koan serve --port 4000

# 3. Get a token
curl -X POST http://localhost:4000/auth/login \
  -H "Content-Type: application/json" \
  -d '{"username": "admin", "password": "your-password"}'

# Response:
# { "access_token": "eyJ...", "refresh_token": "...", "expires_in": 900 }

# 4. Use the token
curl http://localhost:4000/graphql \
  -H "Authorization: Bearer eyJ..." \
  -H "Content-Type: application/json" \
  -d '{"query": "{ libraryStats { trackCount } }"}'
```

## CLI authentication

```bash
# Login to a running koan server (stores refresh token in system keyring)
koan auth login http://localhost:4000

# Token auto-refreshes — no need to login again until the refresh token expires (30 days)
```

## User management

```bash
# List users
koan auth list-users

# Create a user with a specific role
koan auth create-user --username alice --role user
# (prompts for password)

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
# This resets the password for an existing user if the username already exists
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

Subsonic REST endpoints use their own auth (username + token/salt or plain password). This is separate from JWT auth and uses the same `users` table. The Subsonic auth flow is handled automatically by existing Subsonic clients.
