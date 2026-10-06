- **`koan scan --force-remove`** — deletes stale tracks even when the proportion missing trips the mount-failure brake, for the case where the files really were deleted. It lifts that one check and nothing else: a folder yielding no audio files is still left alone, and a path that cannot be stat'd is still not "gone". The run announces itself up front and lists what it removed.

