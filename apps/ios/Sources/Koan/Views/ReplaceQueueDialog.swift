import KoanFFI
import SwiftUI

/// A click on a sleeve among results plays the record, and a queue built up
/// over an evening should not go to a click meant to open it. With anything
/// queued, the person is asked first: play it in place of the queue, or add it.
struct ReplaceQueueDialog: ViewModifier {
    @Binding var album: Int64?

    @Environment(PlayerModel.self) private var player
    @Environment(LibraryModel.self) private var library
    @Environment(Navigator.self) private var nav
    @Environment(EngineMirror.self) private var mirror

    func body(content: Content) -> some View {
        content.confirmationDialog(
            "Replace the queue?",
            isPresented: Binding(get: { album != nil }, set: { if !$0 { album = nil } }),
            titleVisibility: .visible,
            presenting: album
        ) { id in
            Button("Play") {
                Self.play(id, player: player, nav: nav)
            }
            Button("Add to Queue") {
                player.queue([.album(id: id)], .append)
            }
            Button("Cancel", role: .cancel) {}
        } message: { _ in
            Text("Playing this record takes the \(Format.count(Int64(mirror.queue.count), "track")) queued off.")
        }
    }

    /// Opens the record and plays it in place of the queue. The task ends
    /// once it is playing.
    @discardableResult
    static func play(_ id: Int64, player: PlayerModel, nav: Navigator) -> Task<Void, Never> {
        nav.open(album: id)
        return player.queue([.album(id: id)], .replace(startAt: 0, shuffled: false))
    }
}

extension View {
    /// See `ReplaceQueueDialog`.
    func confirmsReplacingQueue(with album: Binding<Int64?>) -> some View {
        modifier(ReplaceQueueDialog(album: album))
    }
}
