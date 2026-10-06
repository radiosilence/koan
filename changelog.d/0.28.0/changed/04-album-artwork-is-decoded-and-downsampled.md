- **Album artwork is decoded and downsampled off the main actor.** `NSImage(data:)` defers the decode until the image is drawn, which put it on the main thread mid-scroll — and embedded artwork is routinely 1500px square for a tile shown at under 200pt. Grid tiles are downsampled to 512px; a cover opened on its own is left alone.

