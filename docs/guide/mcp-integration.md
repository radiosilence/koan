# MCP integration

kōan exposes two tools over the Model Context Protocol. The client reads the GraphQL schema, then drives everything through the `graphql` tool. There are two ways to connect:

- **A kōan server** serves MCP at `/mcp` on its own address, and signs clients in with kōan accounts. This is how Claude on the web, desktop and phone reaches your library and devices.
- **`koan mcp`** runs a player on this machine over stdio, for a desktop client such as Claude Desktop.

## Connecting to a server

The web UI's **Assistants** page has the address to copy and these steps; opening `/mcp` in a browser goes there too. The apps show the same address under Settings → Integrations → Assistants, with a link to that page; the server lists the `koanMcp` extension, and `/rest/koanMcp` gives the address, only once `sharing.public_url` is set. In Claude, open Settings → Connectors → Add custom connector, and enter `https://<your server>/mcp`. Claude opens the server's sign-in page; sign in with your kōan account and approve. Any MCP client that supports OAuth connects the same way.

The server is its own OAuth authorization server, so nothing else is needed in front of it, but it must know the address it is reached at: set `sharing.public_url` (e.g. `https://music.example.com`). Without it the server offers no sign-in, since an address taken from request headers could be chosen by whoever sends them.

What a connection can do:

- It acts as the account that approved it, at that account's role, except that an admin account acts as `user`; `KOAN_MCP_ADMIN=1` lifts that cap.
- Whatever the role, MCP never runs `organizeExecute`, `organizeUndo` or `updateConfig`: nothing reached through it can move, rename or delete a file, or change where the library is. Track titles, tags and share descriptions reach the model, and any of them can carry an instruction. GraphQL still offers those mutations to admins, for tools that manage the library.
- Its tokens are good at `/mcp` only. GraphQL and the web UI refuse them, so the limits above cannot be stepped round by presenting the same token elsewhere.
- Any client may register, under any name it likes. The consent page names a client by the host it returns to, which is the part that cannot be faked: approve only a connection you started, for a host you recognise. `mcp.redirect_hosts` limits registration to the hosts listed (plus this machine, for desktop clients):

```toml
[mcp]
redirect_hosts = ["claude.ai", "claude.com"]
```

A connection lasts as long as its refresh token is used within `auth.refresh_token_ttl` (30 days by default). To end every connection at once, rotate the server's keys with `koan auth regenerate-keys`; clients then register and sign in again. Resetting an account's password ends that account's.

## Running locally (stdio)

Run `koan scan` at least once so the library is indexed, then add kōan to Claude Desktop's config (`~/Library/Application Support/Claude/claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "koan": {
      "command": "koan",
      "args": ["mcp"]
    }
  }
}
```

Claude Desktop does not read your shell's `PATH`, so a `koan` installed by Homebrew or mise needs its full path (`which koan`):

```json
{
  "mcpServers": {
    "koan": {
      "command": "/opt/homebrew/bin/koan",
      "args": ["mcp"]
    }
  }
}
```

Restart Claude Desktop to pick it up.

## Playing on a phone

Connected to a server, the assistant can play music on the koan apps linked to it rather than on the server: it lists them with `clients`, builds a track list with the library queries, and calls `playOnClient`. Ask for "something chill like Polar Bear on my phone" and the queue starts on the iPhone. See [Playing on a linked app](headless-server.md#playing-on-a-linked-app).

## Tools

| Tool | Purpose |
|------|---------|
| `schema_sdl` | The GraphQL schema, so the model knows what it can ask |
| `graphql` | Runs a query or mutation, in-process, against the same schema as the [GraphQL API](graphql-api.md) |

Anything added to the GraphQL API is available over MCP without changes here. A request like "find my 90s electronic albums, pick one at random and queue it" becomes an `albums` query filtered by year and genre, then `addToQueue` and `play`.

`koan mcp` runs at `user` role, so it cannot trigger scans or change the output device unless `KOAN_MCP_ADMIN=1` is set, and never moves files or rewrites config. See [In-process access](authentication.md#in-process-access).
