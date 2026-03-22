import Foundation
import SwiftUI

@Observable
final class PlaybackManager {
    private(set) var nowPlaying: NowPlaying = .empty
    private(set) var queue: [QueueEntry] = []
    private(set) var error: String?

    let serverBaseURL: URL
    private let client: GraphQLClient
    private var pollTimer: Timer?
    private var pollTask: Task<Void, Never>?

    init(serverBaseURL: URL) {
        self.serverBaseURL = serverBaseURL
        self.client = GraphQLClient(baseURL: serverBaseURL)
    }

    // MARK: - Polling

    func startPolling() {
        stopPolling()
        poll()
        pollTimer = Timer.scheduledTimer(withTimeInterval: 1.0, repeats: true) { [weak self] _ in
            self?.poll()
        }
    }

    func stopPolling() {
        pollTimer?.invalidate()
        pollTimer = nil
        pollTask?.cancel()
        pollTask = nil
    }

    private func poll() {
        pollTask?.cancel()
        pollTask = Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                let np = try await client.fetchNowPlaying()
                if !Task.isCancelled {
                    self.nowPlaying = np
                    self.error = nil
                }
            } catch is CancellationError {
                // expected
            } catch {
                if !Task.isCancelled { self.error = error.localizedDescription }
            }
        }
    }

    // MARK: - Queue

    func refreshQueue() {
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                self.queue = try await client.fetchQueue()
            } catch {
                self.error = error.localizedDescription
            }
        }
    }

    // MARK: - Transport

    func playPause() {
        mutate {
            switch self.nowPlaying.state {
            case .playing: try await self.client.pause()
            case .paused: try await self.client.resume()
            case .stopped: break
            }
        }
    }

    func next() { mutate { try await self.client.next() } }
    func previous() { mutate { try await self.client.previous() } }

    func seek(to positionMs: Int64) {
        mutate { try await self.client.seek(positionMs: positionMs) }
    }

    // MARK: - Queue mutations

    func playItem(queueItemId: String) {
        mutate { try await self.client.playItem(queueItemId: queueItemId) }
    }

    func removeFromQueue(queueItemIds: [String]) {
        mutate {
            try await self.client.removeFromQueue(queueItemIds: queueItemIds)
            self.refreshQueue()
        }
    }

    func moveInQueue(queueItemIds: [String], target: String, after: Bool) {
        mutate {
            try await self.client.moveInQueue(
                queueItemIds: queueItemIds, target: target, after: after
            )
            self.refreshQueue()
        }
    }

    func clearQueue() {
        mutate {
            try await self.client.clearQueue()
            self.refreshQueue()
        }
    }

    func toggleFavourite(trackId: Int64) {
        mutate { _ = try await self.client.toggleFavourite(trackId: trackId) }
    }

    // MARK: - Private

    /// Fire-and-forget a mutation, routing errors to `self.error` on MainActor.
    private func mutate(_ body: @escaping @Sendable () async throws -> Void) {
        Task { [weak self] in
            do {
                try await body()
            } catch {
                guard let self else { return }
                await MainActor.run { self.error = error.localizedDescription }
            }
        }
    }
}
