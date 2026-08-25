import KoanFFI
import SwiftUI

/// The one place a new playlist is named.
///
/// Attached to the window rather than to whatever asked, because most of the
/// things that ask are gone by the time the answer is needed: a context menu
/// closes the instant you pick from it, and an alert attached to its contents
/// closes with it — silently, which is exactly how **Add to Playlist → New
/// Playlist…** came to do nothing at all. Everything asks by setting
/// `PlaylistsModel.naming`; this presents it, wherever it came from.
struct NewPlaylistAlert: ViewModifier {
    @Environment(PlaylistsModel.self) private var playlists
    @Environment(Navigator.self) private var nav
    @State private var name = ""

    func body(content: Content) -> some View {
        content.alert(
            "New Playlist",
            isPresented: Binding(
                get: { playlists.naming != nil },
                set: { if !$0 { playlists.naming = nil } }
            )
        ) {
            TextField("Name", text: $name)
            Button("Cancel", role: .cancel) { name = "" }
            Button("Create") { create() }
        } message: {
            Text(message)
        }
    }

    private func create() {
        let trackIds = playlists.naming ?? []
        let name = self.name
        self.name = ""
        playlists.naming = nil
        Task {
            guard let created = await playlists.create(named: name, trackIds: trackIds)
            else { return }
            // You made it to put something in it — so go and look at it.
            nav.open(playlist: created.id)
        }
    }

    /// Says how much is going in, because the gesture that asked has already
    /// disappeared and there is nothing else left on screen to say.
    private var message: String {
        switch playlists.naming?.count ?? 0 {
        case 0: "Give it a name. You can drag records and tracks onto it afterwards."
        case 1: "One track goes in. Give the playlist a name."
        case let count: "\(Format.count(Int64(count), "track")) go in. Give the playlist a name."
        }
    }
}

extension View {
    /// Hosts the new-playlist dialog. Once per window, high enough up that no
    /// gesture that opens it can take it away again.
    func newPlaylistAlert() -> some View {
        modifier(NewPlaylistAlert())
    }
}
