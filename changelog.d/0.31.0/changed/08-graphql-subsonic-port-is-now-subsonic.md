- **`[graphql] subsonic_port` is now `[subsonic] port`.** The port for the Subsonic API lived in the GraphQL section, so turning Subsonic on meant a key in each of two sections. `[subsonic] enabled` mounts `/rest/*` on the GraphQL port; `port` adds a dedicated listener.

