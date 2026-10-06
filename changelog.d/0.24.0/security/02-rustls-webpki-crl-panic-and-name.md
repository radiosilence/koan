- **rustls-webpki CRL panic and name-constraint bypasses (RUSTSEC-2026-0104, -0098, -0099, -0049)** — a hostile certificate chain from a Subsonic/Navidrome server could panic the sync client, and URI/wildcard name constraints were mis-evaluated. rustls-webpki is now 0.103.15.

