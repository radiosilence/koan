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

Until kōan is on Google Play, the app is installed from an APK. Each CI run on
the main branch or on a pull request builds one, signed with a development
key, and attaches it to the run as `koan-android-<commit>`. To install it:

1. Open the run under the repository's Actions tab and download the
   `koan-android-<commit>` artifact. GitHub delivers it zipped; unzip it to get
   the `.apk`.
2. Install it with either of these:
   - **From the phone:** open the file on the phone and allow your browser or
     file manager to install unknown apps when Android asks.
   - **From a computer:** with USB debugging on, run
     `adb install koan-android-<commit>.apk`.
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

Output goes through AAudio at the rate Android's mixer runs at, so playback is
gapless but not bit-perfect: Android resamples a track whose rate differs from
the device's. Bit-perfect output to a USB DAC, through Android 14's
bit-perfect mixer mode, is planned. Bluetooth is lossy whatever the app does.

## Devices

The phone is one of your devices, as the others are: it can play on another
device through **Play on** in Now Playing, and the others see it through the
server. Finding devices on the local network by Bonjour is not there yet on
Android; a device added by address under Settings → Devices works.
