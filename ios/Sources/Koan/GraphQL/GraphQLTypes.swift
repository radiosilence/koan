import Foundation

// MARK: - Pagination (Relay-style connections)

struct PageInfo: Decodable {
    let hasPreviousPage: Bool
    let hasNextPage: Bool
}

struct Edge<T: Decodable>: Decodable {
    let node: T
    let cursor: String
}

struct Connection<T: Decodable>: Decodable {
    let edges: [Edge<T>]
    let pageInfo: PageInfo

    var nodes: [T] { edges.map(\.node) }
}

// MARK: - Query response wrappers

struct ArtistsResponse: Decodable { let artists: Connection<Artist> }
struct AlbumsResponse: Decodable { let albums: Connection<Album> }
struct TracksResponse: Decodable { let tracks: Connection<Track> }
struct NowPlayingResponse: Decodable { let nowPlaying: NowPlaying }
struct QueueResponse: Decodable { let queue: [QueueEntry] }
struct LibraryStatsResponse: Decodable { let libraryStats: LibraryStats }

// MARK: - Mutation response wrappers

struct StatusResponse: Decodable {
    let ok: Bool
    let message: String
}

struct PauseResponse: Decodable { let pause: StatusResponse }
struct ResumeResponse: Decodable { let resume: StatusResponse }
struct StopResponse: Decodable { let stop: StatusResponse }
struct NextResponse: Decodable { let next: StatusResponse }
struct PreviousResponse: Decodable { let previous: StatusResponse }
struct SeekResponse: Decodable { let seek: StatusResponse }
struct PlayResponse: Decodable { let play: StatusResponse }

struct QueueMutationResponse: Decodable {
    let ok: Bool
    let message: String
    let addedCount: Int
    let queueItemIds: [String]
}

struct AddToQueueResponse: Decodable { let addToQueue: QueueMutationResponse }
struct ReplaceQueueResponse: Decodable { let replaceQueue: QueueMutationResponse }
struct ClearQueueResponse: Decodable { let clearQueue: StatusResponse }

// MARK: - Library stats

struct LibraryStats: Decodable {
    let totalTracks: Int
    let localTracks: Int
    let remoteTracks: Int
    let cachedTracks: Int
    let totalAlbums: Int
    let totalArtists: Int
}
