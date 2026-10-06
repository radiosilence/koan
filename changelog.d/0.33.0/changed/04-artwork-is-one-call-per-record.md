- **Artwork is one call per record rather than a listing thrown away.** Asking for an album's cover fetched the album's *tracks* to find an id to ask with — a query built and carried across the boundary for one integer, once per tile. The engine resolves it in SQL now, in the same call that returns the bytes.

