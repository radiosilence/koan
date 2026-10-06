- **Playlists sync with Navidrome, both ways.** A playlist made here appears on the server; one made there appears here, contents and order included. Every edit pushes in the background, so nothing waits on the network; a push that never got out leaves the local copy newer, and the next sync notices and sends it. Deleting on either side deletes on the other, because a playlist that comes back after you delete it cannot be deleted at all.

  Only what Subsonic itself carries is stored — name, comment, owner, public, and the ordered song list — so the two sides are the same object rather than koan's idea of one. Where a playlist sits in your sidebar, and how you like to look at it, are the exceptions: those are facts about this machine.

