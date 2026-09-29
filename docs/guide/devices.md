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

## Controlling and moving

Picking a device controls it. This device pauses, and the transport, the queue
and what is playing become that device's until you pick another: skip, seek,
reorder, add an album, favourite the track. Library pages are still this
device's library; what you add goes to the other device's queue by the
server's id for each track.

**Move here** on a row sends what the controlled device is playing (its whole
queue and where it is in the current track) to that row's device, pauses the
source, and controls the destination from then on. On a phone, **Move here** on
*This iPhone* brings the Mac's music to the phone. Tracks only on the source
device, with no server id, stay behind, and the app says how many.

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
