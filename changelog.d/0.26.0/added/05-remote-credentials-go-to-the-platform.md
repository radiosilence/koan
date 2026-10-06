- **Remote credentials go to the platform credential store.** `set_remote_credentials` checks them against the server before writing anything, stores the password in the keychain, and empties the config copy — so signing in once migrates a setup that had it in plaintext. Shared by the CLI and the app, which cannot now disagree about where credentials live.

