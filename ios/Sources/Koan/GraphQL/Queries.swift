enum GQL {
    // MARK: - Library queries

    static let artists = """
        query Artists($search: String, $first: Int, $after: String) {
            artists(search: $search, first: $first, after: $after) {
                edges {
                    node { id name albumCount trackCount }
                    cursor
                }
                pageInfo { hasPreviousPage hasNextPage }
            }
        }
        """

    static let albumsForArtist = """
        query Albums($artistId: Int!, $first: Int, $after: String) {
            albums(artistId: $artistId, first: $first, after: $after) {
                edges {
                    node {
                        id title artistId artistName date codec label
                        trackCount totalDurationMs
                    }
                    cursor
                }
                pageInfo { hasPreviousPage hasNextPage }
            }
        }
        """

    static let tracksForAlbum = """
        query Tracks($albumId: Int!, $first: Int, $after: String) {
            tracks(albumId: $albumId, first: $first, after: $after) {
                edges {
                    node {
                        id title artist albumArtist album albumId artistId
                        disc trackNumber durationMs codec sampleRate bitDepth
                        channels bitrate genre source remoteId isFavourite
                    }
                    cursor
                }
                pageInfo { hasPreviousPage hasNextPage }
            }
        }
        """

    // MARK: - Playback queries

    static let nowPlaying = """
        query NowPlaying {
            nowPlaying {
                state positionMs durationMs queueItemId
                track { title artist album codec sampleRate bitDepth channels durationMs }
            }
        }
        """

    static let queue = """
        query Queue {
            queue {
                queueItemId title artist album codec trackNumber disc durationMs isCurrent
            }
        }
        """

    // MARK: - Stats

    static let libraryStats = """
        query LibraryStats {
            libraryStats {
                totalTracks localTracks remoteTracks cachedTracks totalAlbums totalArtists
            }
        }
        """

    // MARK: - Playback mutations

    static let play = """
        mutation Play($queueItemId: String!) { play(queueItemId: $queueItemId) { ok message } }
        """

    static let pause = """
        mutation Pause { pause { ok message } }
        """

    static let resume = """
        mutation Resume { resume { ok message } }
        """

    static let stop = """
        mutation Stop { stop { ok message } }
        """

    static let next = """
        mutation Next { next { ok message } }
        """

    static let previous = """
        mutation Previous { previous { ok message } }
        """

    static let seek = """
        mutation Seek($positionMs: Int!) { seek(positionMs: $positionMs) { ok message } }
        """

    // MARK: - Queue mutations

    static let addToQueue = """
        mutation AddToQueue($trackIds: [Int!]!) {
            addToQueue(trackIds: $trackIds) { ok message addedCount queueItemIds }
        }
        """

    static let replaceQueue = """
        mutation ReplaceQueue($trackIds: [Int!]!) {
            replaceQueue(trackIds: $trackIds) { ok message addedCount queueItemIds }
        }
        """

    static let clearQueue = """
        mutation ClearQueue { clearQueue { ok message } }
        """
}
