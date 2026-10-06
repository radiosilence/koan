- **`sortBy` and `sortDir` now do something.** They were declared, published in the SDL and to MCP,
  and silently dropped: a client asking for `sortBy: DATE` got DB order and no error.
