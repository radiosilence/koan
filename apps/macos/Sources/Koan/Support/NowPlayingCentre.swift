import AppKit
import KoanFFI
import MediaPlayer

/// Control Center, the media keys, and the Now Playing widget.
///
/// The TUI reaches this through souvlaki plus a hand-rolled CFRunLoop pump,
/// because a terminal process has no run loop to hang it off. An AppKit app
/// does, so it talks to `MediaPlayer` directly and skips that machinery.
///
/// Elapsed time is *pushed, not polled*: the system extrapolates from the
/// position and playback rate it was last given, so this only publishes on
/// state changes rather than every frame. Publishing at the poll rate would
/// fight that extrapolation and make the scrubber stutter.
@MainActor
final class NowPlayingCentre {
    private weak var player: PlayerModel?
    private let art: CoverArtCache

    /// What was last published, so a 10 Hz poll doesn't republish unchanged
    /// metadata to the system.
    private var publishedTrack: Int64?
    private var publishedState: PlayState?
    private var publishedPosition: UInt64 = 0

    init(player: PlayerModel, art: CoverArtCache) {
        self.player = player
        self.art = art
        registerCommands()
    }

    // MARK: - Remote commands

    private func registerCommands() {
        let centre = MPRemoteCommandCenter.shared()

        // Remote command handlers are invoked on MediaPlayer's own queue, so
        // they must not inherit this class's main-actor isolation — touching
        // `player` directly from one traps in the isolation check. Hop first.
        func onMain(_ body: @escaping @MainActor (PlayerModel) -> Void) -> (MPRemoteCommandEvent) -> MPRemoteCommandHandlerStatus {
            { [weak self] _ in
                Task { @MainActor in
                    guard let player = self?.player else { return }
                    body(player)
                }
                return .success
            }
        }

        centre.playCommand.addTarget(handler: onMain { $0.resume() })
        centre.pauseCommand.addTarget(handler: onMain { $0.pause() })
        centre.togglePlayPauseCommand.addTarget(handler: onMain { $0.togglePlayPause() })
        centre.nextTrackCommand.addTarget(handler: onMain { $0.next() })
        centre.previousTrackCommand.addTarget(handler: onMain { $0.previous() })

        centre.changePlaybackPositionCommand.isEnabled = true
        centre.changePlaybackPositionCommand.addTarget { [weak self] event in
            guard let event = event as? MPChangePlaybackPositionCommandEvent else {
                return .commandFailed
            }
            let ms = UInt64(max(0, event.positionTime) * 1000)
            Task { @MainActor in self?.player?.seek(toMs: ms) }
            return .success
        }
    }

    /// Deliberately `nonisolated`.
    ///
    /// MediaPlayer invokes the request handler on its own queue while encoding
    /// the image. A closure written inside a `@MainActor` method inherits that
    /// isolation whatever the captures are marked, and the runtime check then
    /// traps the first time a track with artwork starts. Building it here, out
    /// of the actor's reach, is what actually removes the isolation.
    private nonisolated static func artwork(for image: NSImage) -> MPMediaItemArtwork {
        MPMediaItemArtwork(boundsSize: image.size) { _ in image }
    }

    // MARK: - Now Playing info

    /// Called from the player's poll. Cheap when nothing has changed.
    func refresh() {
        guard let player else { return }
        let now = player.nowPlaying

        guard let entry = now.entry else {
            if publishedTrack != nil || publishedState != nil {
                MPNowPlayingInfoCenter.default().nowPlayingInfo = nil
                MPNowPlayingInfoCenter.default().playbackState = .stopped
                publishedTrack = nil
                publishedState = nil
            }
            return
        }

        // Republish on a track change, a state change, or a jump the system
        // couldn't have extrapolated — a seek, in other words.
        let drifted = abs(Int64(now.positionMs) - Int64(publishedPosition)) > 2000
        guard entry.trackId != publishedTrack || now.state != publishedState || drifted else {
            return
        }

        var info: [String: Any] = [
            MPMediaItemPropertyTitle: entry.title,
            MPMediaItemPropertyArtist: entry.artist,
            MPMediaItemPropertyAlbumTitle: entry.album,
            MPNowPlayingInfoPropertyElapsedPlaybackTime: Double(now.positionMs) / 1000,
            MPNowPlayingInfoPropertyPlaybackRate: now.state == .playing ? 1.0 : 0.0,
        ]
        if now.durationMs > 0 {
            info[MPMediaItemPropertyPlaybackDuration] = Double(now.durationMs) / 1000
        }
        // Whatever is already loaded — the transport bar is showing this same
        // cover, so it usually is. Fetching here would put a network round trip
        // on the path of every position update.
        if let trackId = entry.trackId, let image = art.cached(.track(trackId)) {
            info[MPMediaItemPropertyArtwork] = Self.artwork(for: image)
        }

        MPNowPlayingInfoCenter.default().nowPlayingInfo = info
        MPNowPlayingInfoCenter.default().playbackState = switch now.state {
        case .playing: .playing
        case .paused: .paused
        case .stopped: .stopped
        }

        publishedTrack = entry.trackId
        publishedState = now.state
        publishedPosition = now.positionMs
    }
}
