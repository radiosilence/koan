- `koan-core`: `SubsonicAuth` splits the credentials and URL signing out of `SubsonicClient`, which constructs blocking `reqwest` clients and so cannot be built inside a tokio runtime. `SubsonicClient::stream_to_file` fetches through `/rest/stream` for servers that implement only that. `GraphQLClient::stream_url` is gone — it was unused and omitted the auth parameters `/rest/*` requires.

