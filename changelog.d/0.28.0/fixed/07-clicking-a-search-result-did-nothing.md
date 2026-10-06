- **Clicking a search result did nothing.** Albums, artists and tracks on the results page all registered the click and stayed put; the same tiles and pills worked in the album and artist browsers, which is what made it look like a hit-testing problem rather than a navigation one.

  The sidebar's lit row was derived from the detail stack, so that an album opened from anywhere lit Albums. Opening one therefore *moved* the sidebar selection — and a `List` writes a moved selection back through its binding. That setter pops to the section root, which is right when you click a sidebar row and wrong when the List is echoing a move the app just made: it threw away the destination that had only just been pushed. The browsers were unaffected because the lit row was already the right one, so nothing moved and nothing echoed.

  The lit row is now the section being browsed and nothing else. Opening an album from search results keeps Results lit, and Back returns you to them.

