# Shuffle and repeat

Shuffle and repeat change which track plays next. Neither moves anything in
the queue: it stays in the order you gave it, and turning either on or off
never rearranges it.

## Shuffle

With shuffle on, each time a track ends or you press Next, koan picks the next
track at random from those that have not played yet. Every track in the queue
plays once before any plays again. A track queued twice counts as one track,
so it gets one turn, not two. Tracks you add while shuffle is on join the
remaining tracks at random places, and a track you remove is not played.

Previous goes back through the tracks in the order they actually played, not
to the track above in the queue. Before the first of them, it starts the
current track again.

A track is marked as played when it starts playing, wherever it sits in the
queue. When you turn shuffle off, the tracks that played keep that mark and
the rest stay queued, and playback carries on down the queue from the current
track. Tracks you skip over by playing one further down are not marked.

The random order is held in memory. After koan restarts with shuffle on, it
draws a new order from the tracks not yet played, and Previous starts from the
track that was playing.

## Repeat

Repeat is off, the queue, or one track. Repeating the queue starts it over
after the last track; with shuffle on, it starts over once every track has
played, in a new random order that does not open with the track that just
ended. Either way, starting over clears the played marks. Repeating one track
plays it again when it ends; Next and Previous still move on.
