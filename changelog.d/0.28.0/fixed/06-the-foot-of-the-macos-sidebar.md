- **The foot of the macOS sidebar was cut off by the transport bar.** The bar is a `safeAreaInset` on the split view, which reserves space in the window but not inside a column's own scroll view. The detail column already compensated; the sidebar never did, so its footer was laid out against a frame whose bottom sits behind the bar — the divider and the first activity row showed, and the progress bar, the cancel button and the library counts underneath them did not. During a sync you got a label with nothing to say how far along it was.

  The sidebar takes the same measured inset the detail column takes, so the footer grows upward from the top of the bar.

