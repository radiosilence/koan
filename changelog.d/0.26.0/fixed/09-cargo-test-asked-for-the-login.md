- **`cargo test` asked for the login keychain password on every run.** A keychain item's ACL is keyed on the reading binary's code signature, and a cargo test binary is unsigned and rebuilt under a fresh hash each compile — so no ACL can match it, and "Always Allow" grants access to a binary that is about to stop existing. `KOAN_NO_KEYCHAIN=1` opts out and `just check` exports it.

