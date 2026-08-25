#if canImport(AppKit)
import AppKit
#else
import UIKit
#endif
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
    /// Which sleeve actually made it into the published info.
    ///
    /// Art arrives after the track does — on a remote library the fetch is an
    /// HTTP round trip — so the first publish for a track carries none, and
    /// without this nothing would ever publish it: the guard below sees an
    /// unchanged track, an unchanged state and no seek, and returns.
    private var publishedArtwork: AlbumArtwork.Source?
    /// The sleeve a fetch has already been started for, so a record with no art
    /// doesn't start one on every tick.
    private var requestedArtwork: AlbumArtwork.Source?

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
    private nonisolated static func artwork(for image: PlatformImage) -> MPMediaItemArtwork {
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

        // Republish on a track change, a state change, a jump the system
        // couldn't have extrapolated — a seek, in other words — or the artwork
        // finally arriving.
        let source = player.currentArtwork
        let artwork = source.flatMap { art.cached($0, size: .tile) }
        let drifted = abs(Int64(now.positionMs) - Int64(publishedPosition)) > 2000
        let artworkArrived = artwork != nil && publishedArtwork != source
        guard entry.trackId != publishedTrack || now.state != publishedState || drifted
            || artworkArrived
        else {
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
        if let image = artwork {
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
        publishedArtwork = artwork == nil ? nil : source

        if artwork == nil, let source {
            fetchArtwork(source)
        }
    }

    /// Load the cover so a later `refresh` finds it.
    ///
    /// The transport bar asks for this same key, but relying on that makes Now
    /// Playing depend on a view being on screen. The cache shares one fetch per
    /// key, so asking twice costs nothing.
    private func fetchArtwork(_ source: AlbumArtwork.Source) {
        guard requestedArtwork != source else { return }
        requestedArtwork = source
        Task {
            _ = await art.image(for: source, size: .tile)
            refresh()
        }
    }
}
