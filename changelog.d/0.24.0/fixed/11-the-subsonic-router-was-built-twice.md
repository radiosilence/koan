- **The Subsonic router was built twice at startup**, each build re-reading two config files and, on macOS, prompting the keychain. Proxied upstream streams also built a fresh HTTP client and re-read the config on every request; both are now resolved once.

