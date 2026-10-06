- **The credential store is asked once per process rather than once per client.** `subsonic_client` builds credentials from scratch and the download queue, radio, sync and sharing each build one, so a single session asked several times over — and being asked five times for one password is indistinguishable from the app being broken.

