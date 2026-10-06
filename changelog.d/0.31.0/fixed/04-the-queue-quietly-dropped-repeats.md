- **The queue quietly dropped repeats.** Adding a track already in the queue added nothing, and a playlist holding the same song twice played it once. The batch fetch behind every queue add took each row out of its map as it matched it, so an id asked for twice came back once.

