# MCP Integration

kōan exposes two tools over the Model Context Protocol. The client reads the GraphQL schema, then drives everything through the `graphql` tool. There are two ways to connect:

- **A kōan server** serves MCP at `/mcp` on its own address, and signs clients in with kōan accounts. This is how Claude on the web, desktop and phone reaches your library and devices.
- **`koan mcp`** runs a player on this machine over stdio, for a desktop client such as Claude Desktop.

## Connecting to a server

In Claude, open Settings → Connectors → Add custom connector, and enter `https://<your server>/mcp`. Claude opens the server's sign-in page; sign in with your kōan account and approve. Any MCP client that supports OAuth connects the same way.

The server is its own OAuth authorization server, so nothing else is needed in front of it, but it must know the address it is reached at: set `sharing.public_url` (e.g. `https://music.example.com`). Without it the server offers no sign-in, since an address taken from request headers could be chosen by whoever sends them.

What a connection can do:

- It acts as the account that approved it, at that account's role, except that an admin account acts as `user`. Track titles, tags and share descriptions reach the model and any of them can carry an instruction; capped, a model cannot be talked into moving files or rewriting config. `KOAN_MCP_ADMIN=1` lifts the cap.
- Any client may register, under any name it likes. The consent page names a client by the host it returns to, which is the part that cannot be faked: approve only a connection you started, for a host you recognise. `mcp.redirect_hosts` limits registration to the hosts listed (plus this machine, for desktop clients):

```toml
[mcp]
redirect_hosts = ["claude.ai", "claude.com"]
```

A connection lasts as long as its refresh token is used within `auth.refresh_token_ttl` (30 days by default). To end every connection at once, rotate the server's keys with `koan auth regenerate-keys`; clients then register and sign in again. Resetting an account's password ends that account's.

## Running locally (stdio)

1. Make sure `koan` is on your PATH (or note the full path from `which koan`).

2. Add to your Claude Desktop config (`~/Library/Application Support/Claude/claude_desktop_config.json`):

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

If kōan isn't on Claude Desktop's PATH (common with Homebrew or mise), use the full path:

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

3. Restart Claude Desktop. You should see kōan in the MCP server list (plug icon).

4. Make sure you've run `koan scan` at least once so your library is indexed.

## Playing on a phone

Connected to a server, the assistant can play music on the koan apps linked to it rather than on the server: it lists them with `clients`, builds a track list with the library queries, and calls `playOnClient`. Ask for "something chill like Polar Bear on my phone" and the queue starts on the iPhone. See [Playing on a linked app](headless-server.md#playing-on-a-linked-app).

## Tools exposed

| Tool | Purpose |
|------|---------|
| `schema_sdl` | Returns the full GraphQL schema so the LLM knows what queries and mutations are available |
| `graphql` | Executes a GraphQL query or mutation against the running player |

The LLM reads the schema first, then constructs whatever queries it needs. This 2-tool design means new features added to the GraphQL API are automatically available to the MCP server without any changes.

## Example prompts

Things you can ask Claude when kōan is connected:

- "Play me some ambient music"
- "What albums do I have by Aphex Twin?"
- "Queue up Tri Repetae but skip the interludes"
- "Pause" / "Skip this" / "What's playing?"
- "Play something like what's on now but more upbeat"
- "Search my library for anything with 'rain' in the title"
- "Switch audio output to my DAC"
- "Save this queue as 'techno friday'" / "Restore my chill mix"
- "Turn on radio mode" / "Star this track"

Claude chains GraphQL operations: "find all my 90s electronic albums, pick one at random, and queue it up" becomes an `albums` query filtered by year and genre, then `addToQueue` and `play`.

## How it differs from the GraphQL API

The MCP server executes GraphQL in-process against the same schema as the [GraphQL API](graphql-api.md). `koan mcp` serves it over stdio at `user` role, so the mutations that move files, rewrite config, trigger scans or change the output device are refused unless `KOAN_MCP_ADMIN=1` is set (see [In-process access](authentication.md#in-process-access)). A server's `/mcp` acts as the signed-in account, capped the same way.
