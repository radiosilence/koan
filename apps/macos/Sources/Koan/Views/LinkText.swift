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

    @Environment(LibraryModel.self) private var library
    @State private var hovering = false

    var body: some View {
        if let target {
            Text(text)
                .font(font)
                .underline(hovering)
                .foregroundStyle(hovering ? AnyShapeStyle(.primary) : AnyShapeStyle(.secondary))
                .lineLimit(1)
                .contentShape(.rect)
                .onHover { hovering = $0 }
                .onTapGesture {
                    switch target {
                    case .artist(let id): library.reveal(artist: id)
                    case .album(let id): library.reveal(album: id)
                    }
                }
                .help("Go to \(text)")
        } else {
            Text(text)
                .font(font)
                .foregroundStyle(.secondary)
                .lineLimit(1)
        }
    }
}

/// Album art that plays the record when clicked, with the affordance only
/// showing on hover so a wall of covers stays a wall of covers.
struct PlayableArtwork: View {
    let albumId: Int64
    var cornerRadius: CGFloat = 6

    @Environment(PlayerModel.self) private var player
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
    /// and the queue command follows once they're in hand.
    private func play() {
        guard !loading else { return }
        loading = true
        let engine = library.engine
        let albumId = self.albumId
        Task {
            let ids = await Task.detached(priority: .userInitiated) {
                ((try? engine.tracks(
                    albumId: albumId, artistId: nil, sort: .album, limit: 500, offset: 0
                )) ?? []).map(\.id)
            }.value
            loading = false
            player.playNow(trackIds: ids)
        }
    }
}
