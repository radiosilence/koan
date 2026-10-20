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
  On a phone, AirPlay has its own button beside it: the system's picker, since
  no app may pick a speaker itself. On the Mac, choose an AirPlay speaker in
  System Settings or Control Centre; playing to the system default, the music
  moves to the speaker chosen there, from where it was. On a phone or an Apple TV the route
  is the only audio device, so Output appears only once an amplifier answers
  (Now Playing looks for them on opening) or while another device is
  controlled.

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
  A phone still awake in the background is refused the audio while another
  app is playing, and shows the same kind of notification instead. Waking and
  notifications need a server that can push to the app
  ([`[push]`](../reference/configuration.md#push)); with any other, a
  suspended phone is listed but takes what was sent only when it is next
  opened. The app asks to show notifications only on a server that can push,
  so elsewhere a phone refused the audio shows nothing either, unless
  notifications were allowed for kōan in iOS Settings.
- **Anyone's device on this network.** Apps find each other over Bonjour and
  connect directly, with no server involved and whoever is signed in. A device
  playing from a different server can be controlled but not sent music, since
  its track ids mean nothing here. Settings → Devices turns this off for the
  device you are on, and sets how much they may do (below).
  A device signed in to a server names that server in its announcement,
  without the account, so a television not signed in yet can offer it on its
  sign-in page instead of asking for the address to be typed.
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

Your own devices are not limited by any of this. On the network, two devices
signed in to the same kōan server with an API key (an invite, pairing, or a
password sign-in on a server that trades it for a key) prove to each other
that they belong to the same account, and a proven device has the account's
powers, as if it had come through the server: handing it music it has not
synced yet syncs it. A device shared with you proves itself the same way and
gets what a share allows. Each device holds a keypair, made when it signs in,
dropped when it signs out, and kept with its credentials in
`config.local.toml`; the server keeps each public key on the API key that
signed the device in and sends the account's devices each other's, so the
proof holds while the server is out of reach. A list not refreshed from the
server for thirty days is no longer trusted. The two ends sign each other's
fresh nonces when they connect, and every command after is signed for that
connection, as is everything the controlled device reports back: what it is
playing and whether a command arrived. The network is not encrypted, so a
recorded proof, a replayed command or report, or one slipped into the
connection is refused. Reports are signed only between devices that both
know to; an older device's are read unsigned, as before. What is playing can
still be seen by anyone on the network. A device that proves
nothing, because it is older, signed in with a password, or on another
server, is trusted by the setting above.

A device that is found but cannot be reached is not listed: there is nothing
to do with it. On iOS, finding anything on the network needs **Local Network**
allowed for kōan (Settings → Privacy & Security); the picker says so when it
is not.

### The terminal UI

`koan` in a terminal is listed as a device too, named after the machine with
"(terminal)" after it, and its own Play on (`C`) controls the others; see
[The terminal UI](terminal-ui.md#play-on). It follows the same settings:
`devices.nearby`, **Visible to devices** and **Devices may** in its Settings
page, and the account it is signed in to. It cannot be woken from elsewhere:
it is reachable while it is running.

### Who is connected

Settings → Devices → **Connected now** lists who is connected to this device
and whom it is connected to: its link to your server and since when, your
account's other devices signed in there, each of which can control this one
through it, and every connection on the local network in either direction. A
device that drives this one keeps it busy, so when kōan uses more battery than
its time on screen explains, this is where to look.

A device on the network that is controlling this one can be disconnected.
Every connection from it ends, and it is listed as disconnected: until you
press **Allow** there, play on it, pick it in Play on, or kōan restarts, it is
not let back in and this device does not connect to it either. Only a device
that proved it is one of yours is held off by its id as well; any other, a
shared one included, is held off by its address alone, so a device giving
your phone's id cannot have the phone shut out with it. A device that
has not proved it is yours, or shared with you, is listed by its address
rather than by the name it gives, and can be refused for good: its address is
kept in `devices.refused` and its connections are hung up as they arrive,
until **Allow** takes it off the list. An address is all there is to know of a
stranger, so one that moves to another address is not refused there. Your
account's own devices are ended by revoking their keys under Server.

### Devices that stop answering

A device is not dropped the moment it goes quiet. For one heartbeat (45
seconds) it reads as reconnecting, so one missed signal does not mark it
asleep. After that it stays listed, as asleep with when it was last seen, only
if this device has used it: controlled it, or sent it music with **Play on** or
**Move here**. A device never used from here is dropped once the heartbeat has
passed, so phones that have moved on, test installs and devices that were
merely on the same network do not pile up. A device heard from again is back
at once, used or not. While nothing can reach it, it can be chosen only
if it can be woken.

- **One a push can wake**, a phone on your account that iOS has suspended,
  can be chosen, and choosing it wakes it. That takes a push key on the
  server; without one, the phone is listed but cannot be woken.
- **One that cannot be woken from here**, such as a stranger's phone on the
  network, is shown asleep and cannot be chosen. A Mac is one of these: macOS
  does not open a quit app for a push, so a Mac whose kōan was quit, or that is
  off or asleep, is shown as not running until someone opens kōan there.

### Keeping a Mac reachable

A Mac can be controlled from your other devices only while kōan is running on
it. With **Keep running in the menu bar** (Settings ▸ Devices ▸ In the
background), closing the window leaves kōan running: it stays signed in to the
server and listening on the network, leaves the Dock and ⌘-Tab, and keeps its
ensō in the menu bar, with what is playing, play and pause, next, **Open kōan**
and **Quit kōan**. Opening the window puts it back in the Dock. Quitting it
from the menu bar takes the Mac out of reach. **Open at login** has macOS open
kōan when you log in, so the Mac is reachable again after a restart; it is
listed, and can be turned off, in System Settings ▸ General ▸ Login Items. The
setting is off by default, and with it off, closing the window quits kōan.

An asleep device you have used stays listed until you forget it: right-click it in the
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

An amplifier given a correction or tuning of its own is sent a stream kōan
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
connection from the amplifier may bring up the firewall prompt. macOS also
asks whether kōan may use the local network, the first time it looks; if
no amplifier or other kōan appears, check System Settings → Privacy &
Security → Local Network and turn kōan on. The iOS app
cannot search for renderers yet, because Apple requires a multicast entitlement
for it.

## Controlling and moving

Picking a device controls it. This device pauses, and the transport, the queue
and what is playing become that device's until you pick another: skip, seek,
reorder, add an album, favourite the track. The system's Now Playing (Control
Centre and the media keys on a Mac, the media notification on Android) shows
that device's track and acts on it too. A command that cannot reach the
controlled device fails and says why; this device does not start playing in
its place. While nothing can reach the controlled device, the Control button
shows a warning sign in the warning colour, before anything is pressed. Pick this device under
**Control** to play here again. Library pages are still this
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

**Move here** is offered only on devices signed in to the same server as this
one, since the music goes by the server's ids for its tracks. One of the
account's devices that has since signed in to another server, as it says when
met on the local network, is controlled but not moved to. A destination asleep
is woken first, as choosing it does: on the network, then with a background
push, while the music plays on where it is. Woken, it takes the music at once;
still asleep, it gets a notification to tap.

A command counts as delivered only once the other device answers that it has
acted on it. Until then the Mac and iOS apps say so: "Waiting for Phone to
wake" while it is queued for a device asleep, and "Couldn't reach Phone", with
the reason, when it was refused or no way through answered. A move waits for
the destination's answer: refused, and the music plays on here; not yet picked
up, and it stays paused here.

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
uses one only where the server lists it. Devices rely on `koanLink`,
`koanDevices`, `koanAck`, `koanShares` and `koanDeviceKeys`; the
[protocol reference](../reference/protocol/protocol.md) specifies each. Each
device's outputs travel with its state, and a server older than the apps drops
them, so the Output menu for another device needs the server updated too.

Settings → Server shows what the server said it is and the extensions it
listed. A server that lists none of them, such as Navidrome, still gets every
Subsonic feature; its devices can find each other only on the local network.
