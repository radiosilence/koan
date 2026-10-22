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
    private let mirror: EngineMirror
    private let art: CoverArtCache

    /// What was last published, so a position tick doesn't republish unchanged
    /// metadata to the system.
    ///
    /// The queue entry and what it names, rather than the library track: a
    /// controlled device's queue can hold tracks this library does not have,
    /// which carry no track id, and an entry it reported without an id is
    /// named by its place, which a new queue reuses for another track.
    private var publishedItem: [String]?
    private var publishedState: PlayState?
    /// The rate the system was told to extrapolate at.
    private var publishedRate = 0.0
    private var publishedPosition: UInt64 = 0
    /// When the position was published, so a later reading can be compared
    /// with where the system has extrapolated it to rather than with where it
    /// started.
    private var publishedAt = ContinuousClock.now
    /// The artwork that made it into the published info.
    ///
    /// Art arrives after the track does — on a remote library the fetch is an
    /// HTTP round trip — so the first publish for a track carries none, and
    /// without this nothing would ever publish it: the guard below sees an
    /// unchanged track, an unchanged state and no seek, and returns.
    private var publishedArtwork: MPMediaItemArtwork?
    /// The artwork for the sleeve on screen, kept until the sleeve changes or
    /// the cache replaces its cover.
    ///
    /// The bitmap cache is an `NSCache`, which iOS empties when the app goes to
    /// the background. A pause from the lock screen republishes, and rebuilding
    /// the artwork from the cache then would publish the track without it.
    private var held: (source: AlbumArtwork.Source, revision: Int, artwork: MPMediaItemArtwork)?
    /// The modes last shown on the shuffle and repeat commands.
    private var publishedShuffle: Bool?
    private var publishedRepeat: RepeatMode?
    /// The sleeve and revision a fetch was last started for, so a record with
    /// no art isn't asked for on every tick.
    private var requestedArtwork: (source: AlbumArtwork.Source, revision: Int)?

    init(player: PlayerModel, mirror: EngineMirror, art: CoverArtCache) {
        self.player = player
        self.mirror = mirror
        self.art = art
        registerCommands()
        // Not a view, so nothing invalidates it — this is the one mechanism
        // there is for that, and the guard below is what keeps a position tick
        // from republishing unchanged metadata to the system.
        mirror.follow { [weak self] in self?.refresh() }
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

        centre.changeShuffleModeCommand.isEnabled = true
        centre.changeShuffleModeCommand.addTarget { [weak self] event in
            guard let event = event as? MPChangeShuffleModeCommandEvent else {
                return .commandFailed
            }
            let on = event.shuffleType != .off
            Task { @MainActor in self?.player?.setShuffle(on) }
            return .success
        }
        centre.changeRepeatModeCommand.isEnabled = true
        centre.changeRepeatModeCommand.addTarget { [weak self] event in
            guard let event = event as? MPChangeRepeatModeCommandEvent else {
                return .commandFailed
            }
            let mode: RepeatMode = switch event.repeatType {
            case .one: .one
            case .all: .queue
            default: .off
            }
            Task { @MainActor in self?.player?.setRepeat(mode) }
            return .success
        }

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
    /// of the actor's reach, is what removes the isolation.
    private nonisolated static func artwork(for image: PlatformImage) -> MPMediaItemArtwork {
        MPMediaItemArtwork(boundsSize: image.size) { _ in image }
    }

    // MARK: - Now Playing info

    /// Cheap when nothing has changed. The system extrapolates the elapsed
    /// time from the rate it was given, so this wants telling exactly when the
    /// playhead stops being predictable — which is when the anchor arrives.
    func refresh() {
        guard let player else { return }
        let now = mirror.playback
        publishModes(shuffle: now.shuffle, repeat: now.repeatMode)
        let positionMs = mirror.playhead.at(within: now.durationMs)

        guard let entry = now.entry else {
            if publishedItem != nil || publishedState != nil {
                MPNowPlayingInfoCenter.default().nowPlayingInfo = nil
                MPNowPlayingInfoCenter.default().playbackState = .stopped
                publishedItem = nil
                publishedState = nil
                publishedArtwork = nil
                held = nil
                requestedArtwork = nil
            }
            return
        }

        // Republish on a track change, a state change, a jump the system
        // couldn't have extrapolated — a seek, in other words — or the artwork
        // finally arriving.
        let source = player.currentArtwork
        let revision = source.map(art.revision(of:))
        if held?.source != source || held?.revision != revision { held = nil }
        if requestedArtwork?.source != source { requestedArtwork = nil }
        if let source, let revision, held == nil {
            if let image = art.cached(source, size: .tile) {
                held = (source, revision, Self.artwork(for: image))
            } else if !art.isAbsent(source) {
                fetchArtwork(source, revision: revision)
            }
        }
        let artwork = held?.artwork
        let item = [entry.queueItemId, entry.title, entry.artistCredit.text, entry.album, entry.albumArtistCredit.text]
        // A transport that plays but a playhead that holds still — waiting for
        // a download, or run dry at a track's end — is rate 0 to the system.
        let rate = now.state == .playing && mirror.playhead.playing && !now.waiting ? 1.0 : 0.0
        var extrapolated = Int64(publishedPosition)
            + Int64(publishedRate * ((ContinuousClock.now - publishedAt) / .milliseconds(1)))
        if now.durationMs > 0 { extrapolated = min(extrapolated, Int64(now.durationMs)) }
        let drifted = abs(Int64(positionMs) - extrapolated) > 2000
        guard item != publishedItem || now.state != publishedState || rate != publishedRate || drifted
            || artwork !== publishedArtwork
        else {
            return
        }

        var info: [String: Any] = [
            MPMediaItemPropertyTitle: entry.title,
            MPMediaItemPropertyArtist: entry.artistCredit.text,
            MPMediaItemPropertyAlbumTitle: entry.album,
            MPNowPlayingInfoPropertyElapsedPlaybackTime: Double(positionMs) / 1000,
            MPNowPlayingInfoPropertyPlaybackRate: rate,
        ]
        // A record naming no album artist is credited "Unknown Artist" on
        // screen; the system is told nothing rather than that.
        if !entry.albumArtist.isEmpty {
            info[MPMediaItemPropertyAlbumArtist] = entry.albumArtistCredit.text
        }
        if now.durationMs > 0 {
            info[MPMediaItemPropertyPlaybackDuration] = Double(now.durationMs) / 1000
        }
        if let artwork {
            info[MPMediaItemPropertyArtwork] = artwork
        }

        MPNowPlayingInfoCenter.default().nowPlayingInfo = info
        MPNowPlayingInfoCenter.default().playbackState = switch now.state {
        case .playing: .playing
        case .paused: .paused
        case .stopped: .stopped
        }

        publishedItem = item
        publishedState = now.state
        publishedRate = rate
        publishedPosition = positionMs
        publishedAt = .now
        publishedArtwork = artwork
    }

    /// Show the play mode on the system's shuffle and repeat controls, when it
    /// moved.
    private func publishModes(shuffle: Bool, repeat mode: RepeatMode) {
        guard shuffle != publishedShuffle || mode != publishedRepeat else { return }
        let centre = MPRemoteCommandCenter.shared()
        centre.changeShuffleModeCommand.currentShuffleType = shuffle ? .items : .off
        centre.changeRepeatModeCommand.currentRepeatType = switch mode {
        case .off: .off
        case .queue: .all
        case .one: .one
        }
        publishedShuffle = shuffle
        publishedRepeat = mode
    }

    /// Load the cover and publish it.
    ///
    /// The transport bar asks for this same key, but relying on that makes Now
    /// Playing depend on a view being on screen. The cache shares one fetch per
    /// key, so asking twice costs nothing.
    private func fetchArtwork(_ source: AlbumArtwork.Source, revision: Int) {
        guard requestedArtwork?.source != source || requestedArtwork?.revision != revision else { return }
        requestedArtwork = (source, revision)
        Task {
            let image = await art.image(for: source, size: .tile)
            // A fetch that lands after the sleeve changed, after playback
            // stopped or after art was found another way is dropped.
            guard requestedArtwork?.source == source, requestedArtwork?.revision == revision, held == nil else {
                return
            }
            if let image {
                // Held straight away, since the cache can drop the bitmap
                // before `refresh` reads it; a cover replaced meanwhile is
                // fetched again by `refresh`.
                if art.revision(of: source) == revision {
                    held = (source, revision, Self.artwork(for: image))
                }
            } else {
                // A record with no art is marked by the cache and left alone;
                // a fetch that failed is tried again.
                guard !art.isAbsent(source) else { return }
                try? await Task.sleep(for: .seconds(30))
                guard requestedArtwork?.source == source, requestedArtwork?.revision == revision, held == nil else {
                    return
                }
                requestedArtwork = nil
            }
            refresh()
        }
    }
}
