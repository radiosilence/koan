- **A GraphQL query could stall audio in every connected Subsonic client.** rusqlite is blocking and
  nothing in the server ran it off the async runtime, so resolvers occupied tokio workers directly.
  Four concurrent `fuzzySearch` calls on a 4-core box took every worker, the `ReaderStream` feeding
  each in-flight `/rest/stream` response stopped producing bytes, and clients dropped the connection
  mid-track. One `triggerScan` did it single-handedly for the length of the scan. Every SQLite call,
  HTTP fetch, tag read and file decode now runs on the blocking pool.
