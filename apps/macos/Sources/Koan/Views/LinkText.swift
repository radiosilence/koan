import KoanFFI
import SwiftUI

/// A name that navigates. Artist and album names should behave the same way
/// wherever they appear — in a grid cell, a track row, a queue header — so the
/// behaviour lives in one place rather than being re-hand-rolled per view.
///
/// Underlines on hover rather than permanently: a library view is mostly names,
/// and colouring or underlining all of them turns it into a ransom note.
struct LinkText: View {
    enum Target {
        case artist(Int64)
        case album(Int64)
    }

    let text: String
    let target: Target?
    var font: Font = .callout
    /// The link is the row's own subject rather than a reference out of it —
    /// an artist in the artists list, not the artist credited on a track.
    var prominent = false

    @Environment(LibraryModel.self) private var library
    @Environment(Navigator.self) private var nav
    @State private var hovering = false

    private func transfer(for target: Target) -> PlayableTransfer {
        switch target {
        case .artist(let id): PlayableTransfer(kind: .artist, id: id, name: text)
        case .album(let id): PlayableTransfer(kind: .album, id: id, name: text)
        }
    }

    var body: some View {
        if let target {
            Text(text)
                .font(font)
                .underline(hovering)
                .foregroundStyle(
                    hovering || prominent ? AnyShapeStyle(.primary) : AnyShapeStyle(.secondary)
                )
                .lineLimit(1)
                .contentShape(.rect)
                .pointerStyle(.link)
                .onHover { hovering = $0 }
                // A row that scrolls or filters away while hovered never sees
                // the exit, so it would come back still underlined.
                .onDisappear { hovering = false }
                .onTapGesture {
                    switch target {
                    case .artist(let id): nav.open(artist: id)
                    case .album(let id): nav.open(album: id)
                    }
                }
                // Dragging the name drags what it names, which is not
                // necessarily what the surrounding row or tile stands for: the
                // artist link on an album tile queues the whole artist, while
                // the artwork beside it queues just that record.
                .draggableTransfer(transfer(for: target))
                .help("Go to \(text)")
        } else {
            Text(text)
                .font(font)
                .foregroundStyle(prominent ? AnyShapeStyle(.primary) : AnyShapeStyle(.secondary))
                .lineLimit(1)
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
        AlbumArtwork(source: .album(albumId), cornerRadius: cornerRadius)
            .overlay {
                if hovering || loading {
                    ZStack {
                        RoundedRectangle(cornerRadius: cornerRadius)
                            .fill(.black.opacity(0.35))
                        if loading {
                            ProgressView()
                                .controlSize(.small)
                                .tint(.white)
                        } else {
                            Image(systemName: "play.circle.fill")
                                .font(.system(size: 34))
                                .foregroundStyle(.white)
                                .shadow(radius: 4)
                        }
                    }
                    .transition(.opacity)
                }
            }
            .animation(.easeOut(duration: 0.12), value: hovering)
            .onHover { hovering = $0 }
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
            let ids = ((try? await engine.tracks(
                albumId: albumId, artistId: nil, sort: .album, limit: 500, offset: 0
            )) ?? []).map(\.id)
            loading = false
            player.playNow(trackIds: ids)
        }
    }
}
