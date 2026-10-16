# Android

kōan on Android is a phone app over the same engine as the Mac and iPhone
apps. Like the iPhone app it plays from a server: sign in to a kōan, Navidrome
or Subsonic server and it browses that library, plays it gaplessly, keeps
playing in the background with the usual media notification, and downloads
what you play so it plays again without a connection. It needs Android 10 or
later on a 64-bit ARM phone.

The pages are the iPhone's: Queue, Library, Search and Settings as tabs, Now
Playing over them, and the same shelves, record and artist pages, playlists,
downloads and history. It is drawn in the kōan theme by default; Settings →
Appearance offers Android's own look instead, in Material You's colours, with
the playing record's accent in place of the wallpaper's.

## Installing the APK

Until kōan is on Google Play, the app is installed from an APK. Each release
on GitHub carries one, `koan-android-arm64.apk`, signed with a development key.
Each CI run on the main branch or on a pull request builds one too, attached to
the run as `koan-android-<commit>`, for trying a change before it is released.
To install it:

1. Download `koan-android-arm64.apk` from the latest release, or open a run
   under the repository's Actions tab and download its `koan-android-<commit>`
   artifact, which GitHub delivers zipped; unzip it to get the `.apk`.
2. Install it with either of these:
   - **From the phone:** open the file on the phone and allow your browser or
     file manager to install unknown apps when Android asks.
   - **From a computer:** with USB debugging on, run
     `adb install koan-android-arm64.apk`.
3. Open kōan and sign in with your server's address, user and password.

Updating is the same: install the newer APK over the old one. Each build is
signed with the key of the machine that built it, so Android refuses to install
a CI build over one you built locally, and the reverse. Uninstall the old one
first. That signs the app out and removes its downloads.

The APK is built for 64-bit ARM, which covers current phones and emulators on
Apple-silicon Macs. To build it yourself, run `just android-apk`; it prints the
path.

## Signing in

Settings → Server signs in with an address, a user and a password or API key.
An invite link from the server's admin, or a `koan://join` link, opens the app
signed in. **Sign in with a code** pairs the phone from a device that is
already signed in, as the Apple TV does: the phone shows a code, and you
approve it from the other device or the web UI.

The `koan.rocks/join/` and `koan.rocks/pair/` links open the app only once the
site publishes the release signing key's fingerprint, which waits on a Play
listing. Until then they open in the browser, and the code is typed by hand.

## Sound

Output goes through AAudio. On Android 14 and later, kōan sends music to a USB
DAC exactly as it is in the file, with no resampling, volume or mixing, when
the DAC can play the track's sample rate and channel count as they are; Android
calls this a bit-perfect mixer. The format badge on Now Playing shows which path
the music takes: **bit-perfect** when the samples reach the DAC unchanged, or
the rate Android's mixer resamples to when it handles the output, because the
DAC does not offer the track's format, the phone is playing through something
other than USB, or the setting is off. Bluetooth is lossy whatever the app does.

While kōan plays bit-perfect, Android keeps other apps' sounds, notifications
included, off the DAC, and the phone's volume buttons may do nothing, since
volume is applied in the mixer; use the DAC's or the headphones' own volume.
**Settings → Playback → Bit-perfect over USB** turns it off, and the system
mixer then handles everything as it does for any other app.

## Devices

The phone is one of your devices, as the others are: it can play on another
device through **Play on** in Now Playing, and the others see it through the
server. On the same Wi-Fi, the phone also finds kōan devices directly and is
found by them, as the Apple apps do over Bonjour, and finds UPnP/DLNA
renderers to play to. Android 17 asks, on first launch, whether kōan may find
devices on the local network; without it neither kind is found, and the
permission can be granted later under the app's Nearby devices permission. A
device added by address under Settings → Devices works where discovery cannot
reach, across subnets or networks that block multicast.

## EQ

Settings → EQ corrects the phone's output with a profile and plays tunings on
top of it, as the iPhone does: AutoEQ's corrections by headphone, offered for
the output in use when its name is one AutoEQ knows; profiles imported from
Equalizer APO, AutoEQ or Wavelet text, WAV impulse responses or a zip of them,
from the file picker, pasted, or shared to kōan from another app; and presets
that hold a correction and a tuning to switch between. A profile's page draws
what it does; a tuning's bands are edited in its table or dragged on the graph,
and a graphic curve is painted there. Nothing is applied until a profile is
chosen. Measurements and squig.link, splitting a combined EQ, targets, and
choosing profiles for other outputs are on the Mac and iPhone only for now.
