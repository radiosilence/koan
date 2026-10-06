- **Sharing a track asked the config file whether there was a password.** The same mistake `koan remote status` had in v0.27: `remote.password` is empty for every keychain-backed sign-in, so "Remote not configured" was the answer on a perfectly good setup. It goes through `subsonic_client` and reports `remote_unavailable()` like everything else.

