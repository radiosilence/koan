- **Play history.** koan never recorded its own plays: `play_history` existed for the radio feature, and its only production writer was the inbound Subsonic scrobble route — i.e. plays arrived when *other* clients scrobbled to koan, and playing a track in koan itself wrote nothing.

  A track is now written to history the moment it starts, not once it has been listened to for long enough. History answers "what did I put on, and in what order"; putting something on and skipping two seconds in is still a thing you did, and a log with a threshold on it is a log with holes in it. How long it was actually heard for is filled in afterwards, from position deltas — so a pause adds nothing and a seek does not credit the stretch it skipped.

  The macOS app gains a **History** section (⌘5) grouped by day. Read-only: rows link through to the album and the artist and carry the usual play/queue menu, but the only thing you can change is the log itself — select and ⌫ to forget entries, the same as queue items.

  Plays of tracks that came from a remote server are scrobbled to it. `SubsonicClient::scrobble` had been written and never called.

