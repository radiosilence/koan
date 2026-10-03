# MCP Integration

`koan mcp` runs kōan as a headless player controlled by Claude Desktop or any other MCP client, with two tools exposed over the Model Context Protocol on stdio. The client reads the GraphQL schema, then drives everything through the `graphql` tool.

## Setup

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

With kōan running as a server (`koan --headless`, MCP over HTTP behind a gateway), the assistant can play music on the koan apps linked to it rather than on the server: it lists them with `clients`, builds a track list with the library queries, and calls `playOnClient`. Ask for "something chill like Polar Bear on my phone" and the queue starts on the iPhone. See [Playing on a linked app](headless-server.md#playing-on-a-linked-app).

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
- "Star this track"

Claude chains GraphQL operations: "find all my 90s electronic albums, pick one at random, and queue it up" becomes an `albums` query filtered by year and genre, then `addToQueue` and `play`.

## How it differs from the GraphQL API

The MCP server executes GraphQL in-process against the same schema as the [GraphQL API](graphql-api.md). `koan mcp` serves it over stdio at `user` role, so the mutations that move files, rewrite config, trigger scans or change the output device are refused unless `KOAN_MCP_ADMIN=1` is set (see [In-process access](authentication.md#in-process-access)). A server started with `--mcp-bind` also serves it over HTTP behind an authenticating gateway; see [Headless Server](headless-server.md).
