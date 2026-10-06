- **The test suite read the developer's own configuration, and asked for their keychain password.** `Config::load()` resolves its directory from `$HOME`, so tests inherited whatever library folders and remote server belonged to whoever ran them. A koan-tui *rendering* test spawned the download queue, which resolved the configured server and reached for the credentials to reach it with — and macOS put up a dialog asking for the login keychain password. The suite then blocked on it: koan-server's tests took 25.52s waiting, and 0.10s when the keychain was disabled.

  Nothing was hardcoded; that is what made it easy to miss. The tests were simply configured as the person running them, and CI never noticed because a runner has no `~/.config/koan/` to inherit. On a developer's machine the same code path would have gone on to fetch tracks from their server.

  Tests point the configuration at a disposable directory now, so they read a config nobody has edited. The ones that need a remote build their own rather than borrowing one.

