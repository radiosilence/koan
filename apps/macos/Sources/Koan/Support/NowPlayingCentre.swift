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

        centre.playCommand.addTarget { [weak self] _ in
            self?.player?.resume()
            return .success
        }
        centre.pauseCommand.addTarget { [weak self] _ in
            self?.player?.pause()
            return .success
        }
        centre.togglePlayPauseCommand.addTarget { [weak self] _ in
            self?.player?.togglePlayPause()
            return .success
        }
        centre.nextTrackCommand.addTarget { [weak self] _ in
            self?.player?.next()
            return .success
        }
        centre.previousTrackCommand.addTarget { [weak self] _ in
            self?.player?.previous()
            return .success
        }

        centre.changePlaybackPositionCommand.isEnabled = true
        centre.changePlaybackPositionCommand.addTarget { [weak self] event in
            guard let event = event as? MPChangePlaybackPositionCommandEvent,
                  let player = self?.player
            else { return .commandFailed }
            player.seek(toMs: UInt64(max(0, event.positionTime) * 1000))
            return .success
        }
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
        if let trackId = entry.trackId, let image = art.art(trackId: trackId) {
            info[MPMediaItemPropertyArtwork] = MPMediaItemArtwork(boundsSize: image.size) { _ in
                image
            }
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
