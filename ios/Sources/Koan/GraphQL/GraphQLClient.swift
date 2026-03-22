import Foundation

actor GraphQLClient {
    private let session: URLSession
    private var baseURL: URL

    init(baseURL: URL, session: URLSession = .shared) {
        self.baseURL = baseURL
        self.session = session
    }

    func setBaseURL(_ url: URL) {
        baseURL = url
    }

    func execute<T: Decodable>(
        query: String,
        variables: [String: GraphQLValue]? = nil
    ) async throws -> T {
        let endpoint = baseURL.appendingPathComponent("graphql")
        var request = URLRequest(url: endpoint)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONEncoder().encode(
            GraphQLRequest(query: query, variables: variables)
        )

        let (data, response) = try await session.data(for: request)

        if let http = response as? HTTPURLResponse, !(200...299).contains(http.statusCode) {
            throw GraphQLClientError.httpError(http.statusCode)
        }

        let gqlResponse = try JSONDecoder().decode(GraphQLResponse<T>.self, from: data)

        if let errors = gqlResponse.errors, let first = errors.first {
            throw GraphQLClientError.graphQL(first.message)
        }

        guard let result = gqlResponse.data else {
            throw GraphQLClientError.noData
        }
        return result
    }

    // MARK: - Typed helpers

    func fetchNowPlaying() async throws -> NowPlaying {
        let data: NowPlayingData = try await execute(query: GQL.nowPlaying)
        return data.nowPlaying.toModel()
    }

    func fetchQueue() async throws -> [QueueEntry] {
        let data: QueueData = try await execute(query: GQL.queue)
        return data.queue.map { $0.toModel() }
    }

    func playItem(queueItemId: String) async throws {
        let _: MutationStatusData = try await execute(
            query: GQL.play, variables: ["queueItemId": .string(queueItemId)]
        )
    }

    func pause() async throws {
        let _: MutationStatusData = try await execute(query: GQL.pause)
    }

    func resume() async throws {
        let _: MutationStatusData = try await execute(query: GQL.resume)
    }

    func stop() async throws {
        let _: MutationStatusData = try await execute(query: GQL.stop)
    }

    func next() async throws {
        let _: MutationStatusData = try await execute(query: GQL.next)
    }

    func previous() async throws {
        let _: MutationStatusData = try await execute(query: GQL.previous)
    }

    func seek(positionMs: Int64) async throws {
        let _: MutationStatusData = try await execute(
            query: GQL.seek, variables: ["positionMs": .int(positionMs)]
        )
    }

    func removeFromQueue(queueItemIds: [String]) async throws {
        let _: MutationStatusData = try await execute(
            query: GQL.removeFromQueue,
            variables: ["queueItemIds": .array(queueItemIds.map { .string($0) })]
        )
    }

    func moveInQueue(queueItemIds: [String], target: String, after: Bool) async throws {
        let _: MutationStatusData = try await execute(
            query: GQL.moveInQueue,
            variables: [
                "queueItemIds": .array(queueItemIds.map { .string($0) }),
                "target": .string(target),
                "after": .bool(after),
            ]
        )
    }

    func clearQueue() async throws {
        let _: MutationStatusData = try await execute(query: GQL.clearQueue)
    }

    func toggleFavourite(trackId: Int64) async throws -> Bool {
        let data: MutationStatusData = try await execute(
            query: GQL.toggleFavourite, variables: ["trackId": .int(trackId)]
        )
        return data.toggleFavourite?.isFavourite ?? false
    }
}

// MARK: - Errors

enum GraphQLClientError: LocalizedError {
    case httpError(Int)
    case graphQL(String)
    case noData

    var errorDescription: String? {
        switch self {
        case .httpError(let code): "HTTP \(code)"
        case .graphQL(let msg): msg
        case .noData: "No data in response"
        }
    }
}

// MARK: - Response -> Model mapping

extension NowPlayingResponse {
    func toModel() -> NowPlaying {
        NowPlaying(
            state: PlaybackState(rawValue: state) ?? .stopped,
            positionMs: positionMs,
            durationMs: durationMs,
            track: track.map {
                NowPlayingTrack(
                    title: $0.title, artist: $0.artist, album: $0.album,
                    codec: $0.codec, sampleRate: $0.sampleRate,
                    bitDepth: $0.bitDepth, channels: $0.channels,
                    durationMs: $0.durationMs
                )
            },
            queueItemId: queueItemId
        )
    }
}

extension QueueEntryResponse {
    func toModel() -> QueueEntry {
        QueueEntry(
            queueItemId: queueItemId, title: title, artist: artist,
            album: album, codec: codec, trackNumber: trackNumber,
            disc: disc, durationMs: durationMs, isCurrent: isCurrent
        )
    }
}
