# Playing on another device

Any kōan app can control another: a phone as the remote for the Mac, the Mac's
queue carried out of the door on the phone, a heart on the phone for what the
Mac is playing. Two buttons, in the Mac's transport bar and in the phone's Now
Playing, make the two choices:

- **Control** (a remote) is which kōan the transport, the queue and Now Playing
  show and command: this device, or another kōan app this one can reach. It
  appears once there is another to pick, and on the phone it is also the
  mini player's button.
- **Output** (a speaker) is where the device in view plays. For this device,
  that is each of its audio devices (built-in speakers, a USB DAC, a display;
  on a phone, the route iOS chose) and the UPnP amplifiers it can see. Picking
  one keeps the queue and transport where they are and moves only the sound.
  AirPlay has its own button beside it: the system's picker, since no app may
  pick a speaker itself. Playing to the system default, the music moves to the
  speaker chosen there, from where it was.

While another device is controlled, Output lists that device's outputs: its
audio devices, the amplifiers it can see, which one it plays through and the
amplifier's volume. Picking one switches that device as its own menu would,
carrying the music over at the same point, playing or paused. Each output shows
its EQ and convolution preset, and the slider button beside it changes it there.
The Output button names the amplifier when that is where the music goes, and
its help reads, for example, "Controlling MacBook · playing through Arcam".

Only the account's own devices, linked through its server, can change another
device's output, its amplifier's volume or a preset. A device found on the local
network without the server can play, pause and change the queue, as before, but
not that.

## Which devices are listed

- **Your devices, anywhere.** Apps signed in to the same account on a kōan
  server see each other through it, on any network. A phone iOS has suspended
  stays listed as asleep: commands wake it, and music sent to it arrives as a
  notification to tap, since iOS does not let an app it woke start playing.
- **Anyone's device on this network.** Apps find each other over Bonjour and
  connect directly, with no server involved and whoever is signed in. A device
  playing from a different server can be controlled but not sent music, since
  its track ids mean nothing here. Settings → Devices turns this off for the
  device you are on, and sets how much they may do (below).
- **Devices by address.** A tailnet carries no Bonjour; add the other device's
  name and port (`mac-mini:5626`) under Settings → Devices.
- **Devices shared with you.** Someone with another account on the same kōan
  server can share a device with yours. It is listed with whose it is, from
  any network, and woken by push as your own devices are.

### What another device may do

Your own devices, signed in to your account, may do anything with each other.
Anyone else's device acts as itself and never gains your account's powers:
whatever it does, nothing reaches your library, the files on disk, your
settings beyond what is playing and where, or your favourites, playlists and
history, and a track it names that this device lacks is not synced for.
Within that, how much it may do depends on how it reaches this device.

- **On the same network**, under Settings → Devices → **Devices on this
  network**:
  - **Full control**, the default: play, pause, skip, seek, the queue, the
    output, the preset and the volume, and moving the music here or away.
    Moving music away goes over the network, or through your server to the
    asker's own devices, never to your other devices. A household network is
    trusted.
  - **Playback only**: play, pause, skip, seek and the queue. Moving music
    away stays on the network. Choose this on a network you share with
    strangers, such as an office or a café.
- **Shared with another account on your server**: on the device itself,
  Settings → Devices → **Shared with other accounts** takes another account's
  username, with the server's accounts offered as you type. That account then
  sees the device from any network, labelled as yours, and has Full control of
  it as if it were on your network, waking it as its own. It can move the
  device's music to its own devices, and pull it back. **Stop sharing** ends
  its control at once.

A share is between accounts on one server, recorded by the server. The device
that is shared is the only one that can share or stop sharing it.

A device that is found but cannot be reached is not listed: there is nothing
to do with it. On iOS, finding anything on the network needs **Local Network**
allowed for kōan (Settings → Privacy & Security); the picker says so when it
is not.

### Devices that stop answering

A device is not dropped the moment it goes quiet. For one heartbeat (45
seconds) it reads as reconnecting, so one missed signal does not mark it
asleep; then it is listed as asleep with when it was last seen. A device heard
from again is back at once. While nothing can reach it, it can be chosen only
if it can be woken.

- **One a push can wake**, a phone on your account that iOS has suspended,
  can be chosen, and choosing it wakes it. That takes a push key on the
  server; without one, the phone is listed but cannot be woken.
- **One that cannot be woken from here**, such as a stranger's phone on the
  network or a Mac that has gone to sleep, is shown asleep and cannot be
  chosen.

An asleep device stays listed until you forget it: right-click it in the
Control menu on the Mac, or press and hold it on iOS, and choose **Forget**.
One this device only knows of, such as a stranger's on the network, is
forgotten here. One of your account's is forgotten by the server too, with the
token it is woken by, and drops off your other devices. Forgetting one another
account shares with you declines the share; its owner can share it again.
Forgetting is not a block otherwise: a device that links or is heard on the network again is listed again.
The server still forgets a device of your account it has not seen for 30 days
on its own.

### Waking a device

Choosing an asleep device wakes it, trying each of these until it answers.
The row says which it is on, and says so if none worked.

1. **The local network**, if the device was seen there within the last minute:
   it may not be suspended yet.
2. **A background push** through your server. iOS delivers these when it sees
   fit. It delays or drops them to save battery and in Low Power Mode, and
   never delivers one to an app that was swiped away.
3. **A notification** on the device, after about six seconds, saying this
   device wants to play there. Tapping it opens kōan, which connects.

A device of another account on your server can be woken too: one shared with
you, from anywhere, and any other when you are behind the same router as it
was when it last connected. The server compares the address each device
connects from, so it treats everyone behind one router as one household: on a
home network that is the point, and nothing else is granted by it, since what
may then be done to the device follows the network and sharing rules above. A
phone on mobile data has another address; sharing covers that.

Each step is logged, on the device choosing (`wake: <name>: Push at +12ms`),
on the server (`wake: wake push for <name> answered by APNs in 140ms`,
`wake: <name> linked 2310ms after its wake push`) and on the phone woken
(`app: push: woken`), so a wake that did not happen shows where it stopped.

iOS gives an app no way to wake itself on a schedule, so a suspended phone
stops announcing itself on the network within seconds of going idle. A phone
that is playing stays reachable, since its audio keeps it running.

The device being controlled and the devices last seen are kept between runs,
so reopening the app shows them at once, still controlling the same device;
each is checked as the app comes to the front.

## Amplifiers and streamers (UPnP)

Network amplifiers and streamers that act as UPnP/DLNA renderers (WiiM,
Yamaha MusicCast, Denon and Marantz HEOS, Cambridge, Arcam and most "network
player" amps) are listed under **Play on** on the Mac, and in the output
device list (`o`) in the TUI, where they are marked `· UPnP`. Kodi with
"Allow remote control via UPnP" turned on, gmrender-resurrect and upmpdcli
are renderers too.

Picking one makes it this device's output, in the same way a USB DAC is, and
kōan goes back to it at the next launch if it is on the network within a few
seconds; playing something or picking another output before then keeps the
music where it is. Until then a session that was playing waits rather than
starting on this device, and plays here if the amplifier does not turn up.

Quitting kōan stops the amplifier. kōan serves what it plays, so the music
cannot outlive the app; stopping it means the amplifier is not left playing
out its buffer and then stalling. The session is saved as playing, so the next
launch picks it up there. The
queue, the transport and history stay on this device, and only the audio goes
to the amplifier. Each track is sent as the original file, so playback is
bit-perfect up to the amplifier's own DAC. Room correction built into the
amplifier, such as Dirac Live, still applies. ReplayGain and fades do not,
since kōan never touches the samples. The volume control in the picker drives
the amplifier's own volume.

An amplifier given an EQ or convolution profile of its own is sent a stream kōan
has processed instead, ReplayGain included; see
[Equalisation and convolution](dsp.md).

- **Gapless** where the renderer accepts the next track in advance
  (`SetNextAVTransportURI`). Otherwise there is a short gap between tracks.
- **Formats the renderer does not list** (often Opus or APE) are skipped and
  marked in the queue with the reason. They play again when you switch back to
  this device.
- **Tracks from a server** play once their download has finished; the next one
  downloads while the current one plays.
- **Controls on the amplifier work.** Pausing or resuming there shows in kōan.
  Stopping there mid-track pauses kōan at that point, and play loads the track
  again from where it stopped.
- **In use by something else**: a renderer already playing for another app is
  marked as such. Picking it takes it over.
- **The progress bar follows the renderer**, which reports its position in
  whole seconds: kōan keeps its own count between readings and corrects it when
  the renderer says otherwise. The bar waits at the start of a track until the
  renderer says it is playing, since some take a second or more to begin.


kōan finds renderers over SSDP and serves each track from a port it opens
only while a renderer is the output. Each track has a random URL of its own,
so nothing else in the library can be fetched from it. On a Mac the first
connection from the amplifier may bring up the firewall prompt. The iOS app
cannot search for renderers yet, because Apple requires a multicast entitlement
for it.

## Controlling and moving

Picking a device controls it. This device pauses, and the transport, the queue
and what is playing become that device's until you pick another: skip, seek,
reorder, add an album, favourite the track. Library pages are still this
device's library; what you add goes to the other device's queue by the
server's id for each track.
The bars beside the playing track show the controlled device's levels, sent
only while they are on screen.

**Move here** on a row pauses the controlled device, then sends what it was
playing (its whole queue, and the point in the current track where it went
silent) to that row's device, and controls the destination from then on. The
destination opens the track at that point rather than starting it and seeking,
so nothing is heard twice and the start of the track is not heard at all. A
track the destination has to download waits until the whole file has arrived. A
paused source arrives paused. On a phone, **Move here** on *This iPhone* brings
the Mac's music to the phone. Tracks only on the source device, with no server
id, stay behind, and the app says how many.

A device that receives music moved to it while it was controlling another
stops controlling it: the music is here now. Move the Mac's music to the phone,
then later move it back from the phone, and the Mac is playing its own music
again, through the amplifier if that was its output. A device told to play
something by another, without a move, keeps controlling whatever it was, so
two devices can still control each other on purpose.

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
| `koanShares` | Sharing a device with other accounts on the server: the grant from the device, the server's accounts to choose from, and shared devices in each grantee's list, relayed with their outputs for the playback set. |
| `koanHistory` | The account's play history as every device's: `/rest/koanHistory` pages the plays and forgettings after a cursor, `/rest/koanForgetPlays` forgets plays for every device, and the link says when the history moved. See [Play history](remote-servers.md#play-history). |
| `koanDevices` | The account's devices sent down each link, commands relayed between them, handoff, Live Activity pushes, and `/rest/koanCommand` for a device whose link is down. Each device's outputs travel with its state; a server older than the apps drops them, so the Output menu for another device needs the server updated too. |

Settings → Server shows what the server said it is and the extensions it
listed. A server that lists neither, such as Navidrome, still gets every
Subsonic feature; its devices can find each other only on the local network.
