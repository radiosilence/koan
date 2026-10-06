- **Opening a record takes seven milliseconds.** Measured end to end in Instruments, from the click to the finished page: the two database reads are 7.4ms, switching the page is 19µs, and the first frame carries the whole record. It was about half a second.

  Most of that half-second was the page loading itself *twice*. The navigator reads a record before it moves to it, and the page then asked again the moment it appeared — the same two queries, but landing while the artwork they had kicked off was still in flight, so the second one took twenty times what the first did and re-rendered a page that was already correct. A record now carries the library version it was read at, which makes asking for what is already on screen free and asking for it after the rows moved a real read.

