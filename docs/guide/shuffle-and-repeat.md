# Shuffle and repeat

Shuffle and repeat change which track plays next. Neither moves anything in
the queue: it stays in the order you gave it, and turning either on or off
never rearranges it. To rearrange the queue or a playlist, use the dice
(below).

Playing something from its play button — a record, an artist, a playlist, a
selection, or a track in a list — turns shuffle and repeat off and starts it
as asked: Play queues it in order, and Shuffle, beside Play on a record's, an
artist's or a playlist's page and in its menu, queues its tracks in a random
order, as the dice would, and plays from the top. The queue shows the order
they will play in, and the dice deals the rest again. Adding to the queue,
Play Next and Add to Queue leave both as they are. This holds on every device
and through the GraphQL API and MCP (`playOnClient`, `playPlaylist`).

## Shuffle

With shuffle on, each time a track ends or you press Next, koan picks the next
track at random from those that have not played yet. Every track in the queue
plays once before any plays again. A track queued twice counts as one track,
so it gets one turn, not two. Tracks you add while shuffle is on join the
remaining tracks at random places, and a track you remove is not played.

Once a track has played for more than a second, Previous starts it again;
pressed again straight away, it goes back a track. It goes back through the
tracks in the order they actually played, not to the track above in the queue.
Before the first of them, it starts the current track again.

A track is marked as played when it starts playing, wherever it sits in the
queue. When you turn shuffle off, the tracks that played keep that mark and
the rest stay queued, and playback carries on down the queue from the current
track. Tracks you skip over by playing one further down are not marked.

The random order is held in memory. After koan restarts with shuffle on, it
draws a new order from the tracks not yet played, and Previous starts from the
track that was playing.

## Shuffling the order itself

On the Mac, iPhone, iPad and Android, a dice in the queue's header and on the
page of a playlist you can edit rearranges the rows themselves, for when you
want to see the order and keep it, or deal again until you like it. In the
queue it shuffles the tracks after the current one, which stays where it is
with everything above it; with no current track, as in a queue never started,
it shuffles the whole queue. A queue that has played to its end still has its
last track as the current one, so there is nothing after it to shuffle. On a
playlist it shuffles every track, and the change is saved and reaches your
server as any edit would.

Each press is one step of undo, so undo takes back one shuffle at a time until
the original order returns. On a Mac that is ⌘Z, or the undo button in the
queue's header; on a phone, an Undo button appears beside the dice while the
last edits were shuffles. The queue's dice is off while shuffle is on, since
shuffle plays in its own order whatever the queue's, and while you control
another device.

## Repeat

Repeat is off, the queue, or one track. Repeating the queue starts it over
after the last track; with shuffle on, it starts over once every track has
played, in a new random order that does not open with the track that just
ended. Either way, starting over clears the played marks. Repeating one track
plays it again when it ends; Next and Previous still move on.
