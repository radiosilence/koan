# Sleep timer

The sleep timer stops playback after 15, 30, 45 or 60 minutes, or at the end of
the track or record that is playing. It fades the music out first and then
pauses, and the queue is left as it was, so pressing play carries on from where
it stopped.

The end of a record is where the album or album artist changes from one track
to the next. With repeat on for a single track, the end of the record never
comes.

The timer belongs to the player, so it holds while iOS suspends the app.
While you control another device, the timer you set is that device's: a phone
controlling a Mac sets the Mac's, and shows the Mac's countdown. A device that
can control this one's playback, on the network or through a share, can set
and cancel it too (see [Playing on another device](devices.md#what-another-device-may-do)).

## The fade

A timer set for a time fades for a tenth of its length, but never less than a
minute or more than five: 90 seconds for 15 minutes, 3 minutes for 30, 5 minutes
for an hour. The fade starts that long before the deadline and reaches silence
at it. A timer for the end of a track or record fades over the last minute of
the track (all of a shorter one) and pauses as the next one begins; for the end
of a record, only the record's last track fades.

The level falls evenly in decibels, from full to 60 dB down, so it sounds like a
steady glide rather than a sudden drop at the end. While it fades, the timer
shows *Fading* in place of its countdown. The fade is a gain on the samples, so
playback is not bit-perfect until it pauses; at full level nothing touches them.
On a UPnP renderer, which is sent the original file, the renderer's own volume
is stepped down instead, about once a second, and put back once it has paused.

Cancelling or changing the timer during the fade brings the level back up over
a second. Pausing by hand during the fade counts as being awake: the timer is
cancelled, and playing again is at full level. Skipping to another track during
an end-of-track fade starts that track at full level.

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
