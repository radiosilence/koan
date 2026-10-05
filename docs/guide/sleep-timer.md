# Sleep timer

The sleep timer stops playback after 15, 30, 45 or 60 minutes, or at the end of
the track or record that is playing. A timer set for a time fades out over a
few seconds and pauses. A timer for the end of a track or record pauses as
the next one begins, without a fade. Either way the queue is left as it was,
so pressing play carries on from where it stopped.

The end of a record is where the album or album artist changes from one track
to the next. With repeat on for a single track, the end of the record never
comes.

The timer belongs to the player, so it holds while iOS suspends the app.
While you control another device, the timer you set is that device's: a phone
controlling a Mac sets the Mac's, and shows the Mac's countdown. A device that
can control this one's playback, on the network or through a share, can set
and cancel it too (see [Playing on another device](devices.md#what-another-device-may-do)).

## Setting it

- **Mac and iOS:** the moon beside the output controls (on iOS, in Now Playing
  next to the lyrics). Pick a choice; while one is set the moon shows the time
  left, or *Track* or *Record*, and the menu offers **Cancel Sleep Timer**.
  Picking another choice replaces the one set.
- **Terminal UI:** `T` steps through 15, 30, 45 and 60 minutes, the end of the
  track, the end of the record, and off. The time left shows on the third line
  of the transport.
- **An assistant or GraphQL:** `setSleepTimerOnClient(minutes: 30)` or
  `setSleepTimerOnClient(endOf: RECORD)` for a linked app, and
  `cancelSleepTimerOnClient`. `clients { sleep { remainingMs endOf } }` shows
  what is set. `setSleepTimer` and `cancelSleepTimer` do the same for the
  server's own player.

A timer that goes off while playback is already paused simply ends. A timer
for a time does not survive quitting the app.
