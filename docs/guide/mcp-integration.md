# MCP Integration

`koan --mcp` runs koan as a headless player controllable by Claude Desktop (or any MCP client). No TUI, no terminal -- just the audio engine and 2 tools exposed over the Model Context Protocol on stdio. The LLM reads the GraphQL schema, then drives everything through one `graphql` tool.

## Setup

1. Make sure `koan` is on your PATH (or note the full path from `which koan`).

2. Add to your Claude Desktop config (`~/Library/Application Support/Claude/claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "koan": {
      "command": "koan",
      "args": ["--mcp"]
    }
  }
}
```

If koan isn't on Claude Desktop's PATH (common with Homebrew or mise), use the full path:

```json
{
  "mcpServers": {
    "koan": {
      "command": "/opt/homebrew/bin/koan",
      "args": ["--mcp"]
||||||| f4afd4f
    }
  }
}
```

3. Restart Claude Desktop. You should see koan in the MCP server list (plug icon).

4. Make sure you've run `koan scan` at least once so your library is indexed.

## Tools exposed

| Tool | Purpose |
|------|---------|
| `schema_sdl` | Returns the full GraphQL schema so the LLM knows what queries and mutations are available |
| `graphql` | Executes a GraphQL query or mutation against the running player |

The LLM reads the schema first, then constructs whatever queries it needs. This 2-tool design means new features added to the GraphQL API are automatically available to the MCP server without any changes.

## Example prompts

Things you can ask Claude when koan is connected:

- "Play me some ambient music"
- "What albums do I have by Aphex Twin?"
- "Queue up Tri Repetae but skip the interludes"
- "Pause" / "Skip this" / "What's playing?"
- "Play something like what's on now but more upbeat"
- "Search my library for anything with 'rain' in the title"
- "Switch audio output to my DAC"
- "Save this queue as 'techno friday'" / "Restore my chill mix"
- "Turn on radio mode" / "Star this track"

Claude chains multiple GraphQL operations together naturally -- "find all my 90s electronic albums, pick one at random, and queue it up" becomes a search -> filter -> add_to_queue -> play sequence.

## How it differs from the GraphQL API

The MCP server executes GraphQL queries in-process (no HTTP round-trip). The schema and capabilities are identical to the [GraphQL API](graphql-api.md) -- same queries, same mutations, same filters.

The main difference is the transport: MCP uses stdio (for Claude Desktop integration), while the GraphQL API uses HTTP (for scripts, web clients, and other tools).
