import KoanFFI
import OSLog
import SwiftUI

/// A name that navigates. Artist and album names should behave the same way
/// wherever they appear — in a grid cell, a track row, a queue header — so the
/// behaviour lives in one place rather than being re-hand-rolled per view.
///
/// Underlines on hover rather than permanently: a library view is mostly names,
/// and colouring or underlining all of them turns it into a ransom note.
struct LinkText: View {
    enum Target: Hashable {
        case artist(Int64)
        case album(Int64)
        /// The artist credited on a track, found when the link is followed. A
        /// queue item knows its track and record but not its artist.
        case artistOfTrack(Int64)
        /// The artist a record is filed under, found when the link is followed:
        /// a run of queue items or playlist entries knows its record by id but
        /// not its album artist.
        case artistOfAlbum(Int64)
    }

    let text: String
    let target: Target?
    var font: Font?
    /// The link is the row's own subject rather than a reference out of it —
    /// an artist in the artists list, not the artist credited on a track.
    var prominent = false
    var lines = 1
    /// Stepped back, as a played row's text is.
    var dimmed = false

    @Environment(Navigator.self) private var nav
    @Environment(LibraryModel.self) private var library
    #if os(iOS)
    /// A list picking rows takes a tap on any of it as a tick.
    @Environment(\.editMode) private var editMode
    #endif
    @State private var hovering = false

    private var rest: AnyShapeStyle {
        if prominent { return KoanTheme.style(.ink, system: .primary) }
        return dimmed ? KoanTheme.style(.muted, system: .tertiary) : KoanTheme.style(.muted, system: .secondary)
    }

    private var followed: Target? {
        #if os(iOS)
        if editMode?.wrappedValue.isEditing == true { return nil }
        #endif
        return target
    }

    /// Only for a target known by its own id; one still to be looked up has
    /// nothing to drag yet.
    private func transfer(for target: Target) -> PlayableTransfer? {
        switch target {
        case .artist(let id): PlayableTransfer(kind: .artist, id: id, name: text)
        case .album(let id): PlayableTransfer(kind: .album, id: id, name: text)
        case .artistOfTrack, .artistOfAlbum: nil
        }
    }

    var body: some View {
        #if os(tvOS)
        // A row is one focusable thing on a television; a link inside it can
        // never be reached, so it reads as the name it is.
        Text(text)
            .font(font ?? .role(.control, system: .callout))
            .foregroundStyle(rest)
            .lineLimit(lines)
        #else
        if let target = followed {
            let link = Text(text)
                .font(font ?? .role(.control, system: .callout))
                .underline(hovering)
                .foregroundStyle(hovering ? KoanTheme.style(.ink, system: .primary) : rest)
                .lineLimit(lines)
                .contentShape(.rect)
                #if os(macOS)
                .pointerStyle(.link)
                #endif
                .pointerHover { hovering = $0 }
                // A row that scrolls or filters away while hovered never sees
                // the exit, so it would come back still underlined.
                .onDisappear { hovering = false }
                .onTapGesture { nav.follow(target, engine: library.engine) }
                .help("Go to \(text)")
            // Dragging the name drags what it names, which is not
            // necessarily what the surrounding row or tile stands for: the
            // artist link on an album tile queues the whole artist, while
            // the artwork beside it queues just that record.
            if let transfer = transfer(for: target) {
                link.dragSource(transfer)
            } else {
                link
            }
        } else {
            Text(text)
                .font(font ?? .role(.control, system: .callout))
                .foregroundStyle(rest)
                .lineLimit(lines)
        }
        #endif
    }
}

extension Navigator {
    /// Goes where a link points, looking up first what a link knows only
    /// through its track or its record. A lookup that lands after the person
    /// has gone somewhere else is dropped rather than pulling them away.
    func follow(_ target: LinkText.Target, engine: KoanEngine) {
        let from = current
        let arrive = { [weak self] (artistId: Int64?) in
            guard let self, self.current == from else { return }
            guard let artistId else {
                Logger(subsystem: "cc.blit.koan", category: "navigation")
                    .warning("no artist found for \(String(describing: target), privacy: .public)")
                return
            }
            self.open(artist: artistId)
        }
        switch target {
        case .artist(let id): open(artist: id)
        case .album(let id): open(album: id)
        case .artistOfTrack(let id):
            Task { arrive((try? await engine.track(trackId: id))??.artistId) }
        case .artistOfAlbum(let id):
            Task { arrive(((try? await engine.album(albumId: id)) ?? nil)?.artistId) }
        }
    }
}

/// Album art that plays the record when clicked and opens it, with the
/// affordance only showing on hover so a wall of covers stays a wall of covers.
///
/// The record is where the tracks are, so that is where a click on its cover
/// leaves you — not the queue, which is behind everything and stays there.
struct PlayableArtwork: View {
    let albumId: Int64
    var cornerRadius: CGFloat = 6

    @Environment(PlayerModel.self) private var player
    @Environment(Navigator.self) private var nav
    @Environment(LibraryModel.self) private var library
    @State private var hovering = false
    @State private var loading = false

    var body: some View {
        AlbumArtwork(source: .album(albumId), cornerRadius: KoanTheme.radius(cornerRadius))
            .overlay {
                if hovering || loading {
                    ZStack {
                        RoundedRectangle(cornerRadius: KoanTheme.radius(cornerRadius))
                            .fill(.black.opacity(0.35))
                        if loading {
                            ProgressView()
                                .controlSize(.small)
                                .tint(.white)
                        } else {
                            KoanIcon(Icon.playMark)
                                .font(.system(size: 34))
                                .foregroundStyle(.white)
                                .koanShadow(0.33, radius: 4)
                        }
                    }
                    .transition(.opacity)
                }
            }
            .animation(.easeOut(duration: 0.12), value: hovering)
            .pointerHover { hovering = $0 }
            .onTapGesture { play() }
            .help("Play album")
    }

    /// Resolving the tracks is the slow half, so it happens off the main actor
    /// and the queue command follows once they're in hand. The move to the
    /// record doesn't wait on it — the click should land immediately.
    private func play() {
        guard !loading else { return }
        loading = true
        let engine = library.engine
        let albumId = self.albumId
        nav.open(album: albumId)
        Task {
            let ids = (try? await engine.trackIds(albumId: albumId, artistId: nil)) ?? []
            loading = false
            player.playNow(trackIds: ids)
        }
    }
}
