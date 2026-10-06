- **A write clears the same key from the other file.** `config.local.toml` wins the merge, so a shared write left shadowed by a local copy silently did nothing. In the other direction it drains machine-scoped keys out of `config.toml`, which cleans up a file an older koan polluted as you use the app.

