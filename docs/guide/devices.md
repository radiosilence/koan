# Playing on another device

Any kōan app can control another: a phone as the remote for the Mac, the Mac's
queue carried out of the door on the phone, a heart on the phone for what the
Mac is playing. The **Play on** button (the laptop-and-phone icon, in the Mac's
transport bar and on the phone's mini player and Now Playing) lists the devices
this one can reach.

## Which devices are listed

- **Your devices, anywhere.** Apps signed in to the same account on a kōan
  server see each other through it, on any network. A phone iOS has suspended
  stays listed as asleep: commands wake it, and music sent to it arrives as a
  notification to tap, since iOS does not let an app it woke start playing.
- **Anyone's device on this network.** Apps find each other over Bonjour and
  connect directly, with no server involved and whoever is signed in. A device
  playing from a different server can be controlled but not sent music, since
  its track ids mean nothing here. Settings → Devices turns this off for the
  device you are on.
- **Devices by address.** A tailnet carries no Bonjour; add the other device's
  name and port (`mac-mini:5626`) under Settings → Devices.

A device that is found but cannot be reached is listed with the reason. On
iOS, finding anything on the network needs **Local Network** allowed for kōan
(Settings → Privacy & Security); the picker says so when it is not.

The device being controlled and the devices last seen are kept between runs,
so reopening the app shows them at once, still controlling the same device;
each is checked as the app comes to the front.

## Controlling and moving

Picking a device controls it. This device pauses, and the transport, the queue
and what is playing become that device's until you pick another: skip, seek,
reorder, add an album, favourite the track. Library pages are still this
device's library; what you add goes to the other device's queue by the
server's id for each track.

**Move here** on a row pauses the controlled device, then sends what it was
playing (its whole queue, and the point in the current track where it went
silent) to that row's device, and controls the destination from then on. The
destination opens the track at that point rather than starting it and seeking,
so nothing is heard twice and the start of the track is not heard at all. A
track the destination has to download waits until the whole file has arrived. A
paused source arrives paused. On a phone, **Move here** on *This iPhone* brings
the Mac's music to the phone. Tracks only on the source device, with no server
id, stay behind, and the app says how many.

## The lock screen

While a phone controls another device it shows a Live Activity on the lock
screen and in the Dynamic Island: what is playing there, a progress bar, and
previous / play-pause / next. iOS reserves the system's Now Playing controls
for the app producing the audio, and playing silence to hold them is what App
Review rejects, so this is the supported way to put a remote on the lock
screen.

The app updates it while it runs. Once iOS suspends the app, the server pushes
each change the controlled device reports, which needs the server's `[push]`
key (see [Configuration](../reference/configuration.md#push)); without one the
activity keeps the last state it was given. Its buttons work either way: they
send the command in one request to the server when the app's link is down.

## What the server has to support

kōan lists its additions to Subsonic as OpenSubsonic extensions, and a client
uses one only where the server lists it:

| Extension | What it is |
|-----------|------------|
| `koanLink` | The app's standing WebSocket at `/rest/koanLink`: the server can command it, and it reports what it is playing. |
| `koanDevices` | The account's devices sent down each link, commands relayed between them, handoff, Live Activity pushes, and `/rest/koanCommand` for a device whose link is down. |

Settings → Server shows what the server said it is and the extensions it
listed. A server that lists neither, such as Navidrome, still gets every
Subsonic feature; its devices can find each other only on the local network.
