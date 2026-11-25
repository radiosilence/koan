# koan-auth

Accounts and signing in for [kōan](https://koan.rocks) and the services built beside it, over SQLite.

- Accounts with three roles (admin, user, readonly), argon2id passwords, a first admin made once, and a last admin who cannot be removed or demoted.
- Sessions stored as the SHA-256 of what the client holds, either presented as they are or rotated on each use, with an OAuth grant's spent token coming back taken as theft.
- API keys and app passwords, so a client keeps a credential of its own rather than the password.
- A password verifier with a ceiling on concurrent argon2 checks and a per-account budget of failures that spares the account's own networks.
- An Ed25519 key, access tokens and invite tokens. An invite token names an account and a mark of its password, so setting a password spends it.
- Per-address limits, the address a request came from behind a proxy, an authenticating proxy's header, and a one-time code that guards making the first admin.
- The protocol of an OAuth 2.1 authorization server, and the registry behind pairing a device that has no keyboard.

Every call is synchronous and takes a `rusqlite::Connection`, so a service keeps its own database and HTTP stack, and draws its own pages. A service on tokio makes these calls on its blocking pool. The tables keep their version in `koan_auth_schema`, apart from the service's own: call `koan_auth::schema::migrate` when the database opens.

Licensed MIT, unlike the rest of kōan.
