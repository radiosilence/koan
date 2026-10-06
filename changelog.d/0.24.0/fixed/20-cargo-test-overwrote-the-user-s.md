- **`cargo test` overwrote the user's real JWT signing key.** `auth`'s keypair tests called
  `generate_keypair()`, which writes to `~/.config/koan/auth/`, so running the test suite rotated the
  live Ed25519 key and invalidated every issued token. Keypair derivation is now split from the
  filesystem write and the tests use the pure form.
