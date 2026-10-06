- **Existing refresh tokens are invalidated.** They are now stored as `sha256(token)`; rows written under the old scheme no longer match. Clients re-login once.

