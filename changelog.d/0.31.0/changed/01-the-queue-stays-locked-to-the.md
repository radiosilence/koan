- **The queue stays locked to the playlist that filled it.** Play a playlist and the queue *is* that playlist; reorder it, remove from it or add to it and the queue follows, quietly. Rearrange the queue yourself, add to it, or let radio extend it and the two part company — from then on the playlist is a document you are editing and the queue is what you are listening to.

  Locked is derived rather than tracked: queue items carry the playlist row they came from, so the queue is locked exactly when its entries are that playlist's, in that order. Nothing to keep in sync, nothing to persist, and it cannot get stuck — a queue that stops matching stops being locked, and one that matches again is locked again. Playing a playlist shuffled scrambles the order on purpose, so that queue is not locked, which is the right answer rather than a special case.

  Records lock too, and need no provenance to do it: an album *is* an ordered set of tracks, so the queue being that album is a question about what the queue holds — which means it still answers for a queue restored from a previous session, where nothing remembers what was played. A record cannot be edited, so there is nothing to follow; it just says what you are listening to.

  The queue says so: while it is locked it is headed **Playing *<name>***, with the playlist's mosaic or the record's sleeve beside it, and goes back to plain **Queue** the moment it is not. A rule this quiet has to be visible, or the first silent update reads as koan moving things on its own.


