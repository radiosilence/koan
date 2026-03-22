import Foundation

// MARK: - Request / Response envelope

struct GraphQLRequest: Encodable {
    let query: String
    let variables: [String: GraphQLValue]?
}

enum GraphQLValue: Encodable {
    case string(String)
    case int(Int64)
    case bool(Bool)
    case array([GraphQLValue])

    func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case .string(let s): try container.encode(s)
        case .int(let i): try container.encode(i)
        case .bool(let b): try container.encode(b)
        case .array(let a): try container.encode(a)
        }
    }
}

struct GraphQLResponse<T: Decodable>: Decodable {
    let data: T?
    let errors: [GraphQLError]?
}

struct GraphQLError: Decodable, LocalizedError {
    let message: String
    var errorDescription: String? { message }
}

// MARK: - Response data shapes

struct NowPlayingData: Decodable {
    let nowPlaying: NowPlayingResponse
}

struct NowPlayingResponse: Decodable {
    let state: String
    let positionMs: UInt64
    let durationMs: UInt64?
    let track: NowPlayingTrackResponse?
    let queueItemId: String?
}

struct NowPlayingTrackResponse: Decodable {
    let title: String
    let artist: String
    let album: String
    let codec: String
    let sampleRate: UInt32
    let bitDepth: UInt16
    let channels: UInt16
    let durationMs: UInt64
}

struct QueueData: Decodable {
    let queue: [QueueEntryResponse]
}

struct QueueEntryResponse: Decodable {
    let queueItemId: String
    let title: String
    let artist: String
    let album: String
    let codec: String?
    let trackNumber: Int?
    let disc: Int?
    let durationMs: UInt64?
    let isCurrent: Bool
}

struct StatusResponse: Decodable {
    let ok: Bool
    let message: String
}

struct ToggleFavouriteResponse: Decodable {
    let id: Int64
    let isFavourite: Bool
}

/// Generic wrapper that decodes whichever mutation key is present.
/// Each mutation method only cares about its own key, and callers
/// mostly discard the result, so this uses decodeIfPresent for all keys.
struct MutationStatusData: Decodable {
    let toggleFavourite: ToggleFavouriteResponse?

    enum CodingKeys: String, CodingKey {
        case play, pause, resume, stop, next, previous, seek
        case removeFromQueue, moveInQueue, clearQueue, toggleFavourite
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        toggleFavourite = try c.decodeIfPresent(ToggleFavouriteResponse.self, forKey: .toggleFavourite)
        // Status keys (play/pause/etc.) are intentionally not stored --
        // callers discard them, and decoding succeeds as long as the
        // container has any recognized key.
    }
}
