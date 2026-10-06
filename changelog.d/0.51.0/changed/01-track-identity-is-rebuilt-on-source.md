- **Track identity is rebuilt on source rows.** Each file and each server entry now keeps its own tags in a row of its own, and a track's names, path and server id are derived from them, the file's first. One function decides which file and which server entry are the same track, and it runs whenever either's tags change. This replaces eight separate matching and repair passes, and fixes the problems they shared:
  - A track held both on disk and on a server no longer changes between the file's names and the server's on every sync and rescan. A tag corrected in the file is no longer put back by the next sync.
  - Correcting a file's tags pairs it with its server copy, or splits it from one it no longer matches, whatever the artist credit says.
  - Names match whatever their case or Unicode form, so `SIGUR RÓS` on a server is `Sigur Rós` on disk.
  - A file and its server copy are not paired when the server has two candidates; they used to be paired with whichever came first.
  - A moved file takes over the server copy its old path held, along with its history.
  - Merging two rows keeps the older one's id and history, and the koan server's uid.
  - Two editions of a record with the same title are two albums when their MusicBrainz release ids differ, and a release id is no longer rewritten by whichever file was scanned last.
  - A record the server names differently from the files is one album, holding the server's album id, with the tracks only the server has listed alongside the files.
  - Artist names match whatever their case or Unicode form, so one act no longer appears twice.
  - A server's track number fills in one the file lacks.
  - A database error while matching is reported rather than adding a duplicate.

  Upgrading builds the source rows from the existing library, so the first scan afterwards reads every file again and the first sync walks the whole server.

