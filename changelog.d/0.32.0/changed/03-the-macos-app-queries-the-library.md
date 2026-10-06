- **The macOS app queries the library instead of copying it.** It used to load every album and every artist at launch, narrow those copies in Swift and index them so search could resolve ids against them -- three shapes of the same five thousand rows, held to serve views that read none of them directly. Now a section asks the engine what it should be showing and shows exactly that; narrowing and sorting happen in SQL.

  Nothing is paged. This is an in-process call rather than a wire, so a listing arrives whole: the scrollbar tells the truth about how long the library is, and one flick reaches the end of it.

  The bugs this closes are the ones that came from the copy existing: a section showing a library the database no longer has, and a cold launch showing an empty one because the load lived somewhere the second window never reached. There is no load to have forgotten to do.

  `AlbumSort::Random` now takes a seed, so narrowing a shuffled listing narrows the shuffle you are looking at instead of dealing a new one on every keystroke. A new seed is a new shuffle, which is what the reshuffle button asks for.

