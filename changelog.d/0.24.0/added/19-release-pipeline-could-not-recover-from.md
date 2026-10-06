- **Release pipeline could not recover from a partial crates.io publish.** The idempotency guard used
  `curl -sf` against the crates.io API, which returns 403 to curl's default User-Agent under its
  data-access policy — so the guard never fired and a retry after a partial publish always failed on
  "crate version already uploaded". The only escape was another version bump, which is what v0.23.2 and
  v0.23.3 were. Replaced the hand-rolled loop with `cargo publish --workspace`, which orders by
  dependency and waits for the index itself, and dropped `--no-verify` so the packaged crate is
  actually built.
