- **Nothing the engine logged was written down when it was hosted by the app.** koan-ffi never installed a logger, so every warning in koan-core — a favourite that could not reach the server, a file that would not decode — was discarded. It writes to `~/.config/koan/koan.log`, the same file the CLI uses.

