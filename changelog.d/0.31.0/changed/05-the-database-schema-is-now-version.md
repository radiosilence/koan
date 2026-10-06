- **The database schema is now version 2, and older koan builds will refuse to open it.** That is the point of the version: a build that does not know about `playlists` must not write to a library that has them. Upgrade rather than downgrading.

