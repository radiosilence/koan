- **All remote downloads share one implementation** (`koan-core/src/remote/download.rs`) — there were three copies of "stream bytes to disk with progress" and only one wrote to a temp file and verified the result.

