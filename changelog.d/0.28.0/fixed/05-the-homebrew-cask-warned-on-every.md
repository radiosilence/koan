- **The Homebrew cask warned on every `brew` command.** `depends_on macos: ">= :tahoe"` is a deprecated string comparison; the bare symbol has always meant "that version or newer", so the requirement is unchanged. The workflow that regenerates the cask on release is fixed too, not just the published tap.

