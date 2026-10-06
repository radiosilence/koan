- **A sync never recorded the album's or the artist's id on the server.** The track's was kept and theirs was dropped, so all 5,800 albums in a synced library had a null `remote_id`. The server keys stars, shares and cover art off those ids, which left koan able to name an album but not refer to it. A full sync backfills them.

