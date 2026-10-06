- **The Subsonic REST API now has its own credentials and is off by default.** It previously authenticated with `remote.username` plus the `[remote]` password — the user's actual Navidrome account password — and `/rest/*` was mounted outside the JWT middleware, so `auth_enabled = true` did nothing for the surface that streams every file in the library. The protocol sends `md5(password + salt)` with a client-chosen salt over whatever transport the client picked, so one captured `/rest/ping` on the LAN yielded a digest to crack offline — and cracking it owned the remote music account too.

  Migration:

  ```bash
  koan subsonic setup     # generates a 256-bit secret, stores it in the keychain, prints it once
  ```

  Then re-point your Subsonic clients at the new username (`koan` by default) and secret. `koan subsonic status` shows the current state, `koan subsonic disable` turns it back off. Nothing is served at `/rest/*` until you run setup.

