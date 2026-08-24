import KoanFFI
import SwiftUI

/// Play button that appears on a row when the pointer is over it.
///
/// A one-click way to play something without competing with the row's own
/// selection — which is the trouble with putting gestures on rows at all.
/// Visible only on hover, so a list of names stays a list of names.
struct RowPlayButton: View {
    let playable: Playable
    let visible: Bool
    /// Play this list from here, rather than the row on its own. A track's
    /// play button should start the album at that track and keep the rest
    /// behind it, which is what pressing play on a track means.
    var inContext: (trackIds: [Int64], startAt: Int)?

    @Environment(PlayerModel.self) private var player
    @Environment(Navigator.self) private var nav
    @Environment(LibraryModel.self) private var library
    @State private var loading = false

    var body: some View {
        Button {
            play()
        } label: {
            Image(systemName: "play.circle.fill")
                .font(.system(size: 15))
                .foregroundStyle(.tint)
        }
        .buttonStyle(.plain)
        .opacity(visible || loading ? 1 : 0)
        .allowsHitTesting(visible || loading)
        .help("Play \(playable.name)")
    }

    /// Replaces the queue. Resolution happens off the main actor — an artist is
    /// a lot of rows.
    private func play() {
        guard !loading else { return }
        if let context = inContext {
            player.playNow(trackIds: context.trackIds, startingAt: context.startAt)
            nav.showQueueWhenReady(watching: player)
            return
        }
        loading = true
        let engine = library.engine
        let playable = self.playable
        Task {
            let ids = await playable.trackIds(using: engine)
            loading = false
            player.playNow(trackIds: ids)
            nav.showQueueWhenReady(watching: player)
        }
    }
}
