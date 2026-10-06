- **The window wash cost a quarter of a core to sit still.** It was drawn twice — once on the window, once on the page on top of it — and the page's copy sat on an opaque ground hiding the window's, which kept animating behind it for no one. There is one now, on the window, which is the one that mirrors out under the sidebar and toolbar.

  The one that remains no longer redraws on a timer. It moved its blur through scale, rotation and offset twenty times a second, and each tick invalidated layout: a full window Auto Layout pass, 20 Hz, whether or not anything had changed. The drift is handed to CoreAnimation instead, which runs it off the main thread and smoother for it. Stopping playback now settles the wash back to rest over a couple of seconds rather than freezing it mid-breath.

