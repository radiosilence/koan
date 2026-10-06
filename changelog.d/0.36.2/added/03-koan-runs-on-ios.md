- **koan runs on iOS.** The same engine, models and pages as the Mac app, in a phone's shell: a tab bar (Queue, Library, Settings, Search), a mini player above it, and a full-screen Now Playing with the seek bar, lyrics in place of the sleeve, radio, the output format and an AirPlay picker. Every page stands in the playing record's wash, as the Mac's window does. An iPad with room for a sidebar gets the Mac's layout instead; the choice follows the width, not the device. `just ios-run` builds it for a simulator. iOS 26 or later.

  Output goes through RemoteIO, from the same `engine.rs` the Mac uses; the two differ only in which output unit they open and whether a device can be named. The decode pipeline, timeline, gapless cursor and teardown are shared rather than rewritten. What iOS costs is the bit-perfect claim: everything crosses the system mixer, so koan plays at whatever rate the session settles on.

  Built for a battery. The spectrum analyser behind the playing bars stops while the app is in the background, runs at no more than 60fps on a phone, and at 30 in Low Power Mode, which also stills the wash's drift and the bars. RemoteIO is asked for a 93ms buffer, so the render thread wakes a twentieth as often as the default.

  A phone call pauses playback, and so does unplugging headphones. After an interruption koan resumes only when iOS says it should.
