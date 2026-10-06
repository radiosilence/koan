- **Subsonic playlist endpoints serve real playlists.** `getPlaylists`, `getPlaylist`, `createPlaylist` and `deletePlaylist` used to fake playlists out of saved queues; they now read and write the real thing, and `updatePlaylist` — add and remove members by index — works at last.

