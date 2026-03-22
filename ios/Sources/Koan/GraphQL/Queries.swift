import Foundation

// MARK: - Query strings

enum GQL {
    static let libraryStats = """
    query LibraryStats {
        libraryStats {
            totalTracks
            localTracks
            remoteTracks
            cachedTracks
            totalAlbums
            totalArtists
        }
    }
    """

    static let artists = """
    query Artists($search: String, $favouritesOnly: Boolean!, $after: String, $first: Int) {
        artists(search: $search, favouritesOnly: $favouritesOnly, after: $after, first: $first) {
            pageInfo { hasPreviousPage hasNextPage }
            edges {
                cursor
                node {
                    id
                    name
                    albumCount
                    trackCount
                }
            }
        }
    }
    """

    static let albums = """
    query Albums($artistId: Int, $search: String, $favouritesOnly: Boolean!, $after: String, $first: Int) {
        albums(artistId: $artistId, search: $search, favouritesOnly: $favouritesOnly, after: $after, first: $first) {
            pageInfo { hasPreviousPage hasNextPage }
            edges {
                cursor
                node {
                    id
                    title
                    artistId
                    artistName
                    date
                    codec
                    trackCount
                    totalDurationMs
                }
            }
        }
    }
    """

    static let tracks = """
    query Tracks($albumId: Int, $artistId: Int, $search: String, $favouritesOnly: Boolean!, $after: String, $first: Int) {
        tracks(albumId: $albumId, artistId: $artistId, search: $search, favouritesOnly: $favouritesOnly, after: $after, first: $first) {
            pageInfo { hasPreviousPage hasNextPage }
            edges {
                cursor
                node {
                    id
                    title
                    artist
                    albumArtist
                    album
                    albumId
                    artistId
                    disc
                    trackNumber
                    durationMs
                    codec
                    sampleRate
                    bitDepth
                    genre
                    isFavourite
                }
            }
        }
    }
    """

    static let favourites = """
    query Favourites($after: String, $first: Int) {
        favourites(after: $after, first: $first) {
            pageInfo { hasPreviousPage hasNextPage }
            edges {
                cursor
                node {
                    id
                    title
                    artist
                    album
                    albumId
                    disc
                    trackNumber
                    durationMs
                    codec
                    isFavourite
                }
            }
        }
    }
    """

    static let toggleFavourite = """
    mutation ToggleFavourite($trackId: Int!) {
        toggleFavourite(trackId: $trackId) {
            id title artist album isFavourite
        }
    }
    """

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
}

// MARK: - Response wrappers

struct LibraryStatsResponse: Decodable {
    let libraryStats: LibraryStats
}

struct ArtistsResponse: Decodable {
    let artists: Connection<Artist>
}

struct AlbumsResponse: Decodable {
    let albums: Connection<Album>
}

struct TracksResponse: Decodable {
    let tracks: Connection<Track>
}

struct FavouritesResponse: Decodable {
    let favourites: Connection<Track>
}

struct ToggleFavouriteResponse: Decodable {
    let toggleFavourite: Track
}

struct QueueMutationResult: Decodable {
    let ok: Bool
    let message: String
    let addedCount: Int
    let queueItemIds: [String]
}

struct QueueMutationResponse: Decodable {
    // Keyed by whichever mutation was called — decoder ignores unknown keys.
    let addToQueue: QueueMutationResult?
    let replaceQueue: QueueMutationResult?

    var result: QueueMutationResult? { addToQueue ?? replaceQueue }
}
