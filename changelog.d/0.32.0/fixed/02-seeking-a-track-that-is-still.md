- **Seeking a track that is still downloading stays in the stream.** It used to reopen the partial file as an ordinary file, decoding whatever bytes happened to be on disk and ending the track early. Seeking now stays inside the download and lands anywhere already fetched, forwards or back.

