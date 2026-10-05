import KoanFFI
import SwiftUI

/// What is on this device: the records with files here, fully there first,
/// each with a bar showing how much of it is, and their artists. What a phone
/// can play with no signal.
struct OnDeviceView: View {
    @Environment(LibraryModel.self) private var library

    var body: some View {
        ShelfView(
            title: "Downloaded",
            artists: library.visibleOnDeviceArtists,
            albums: library.visibleOnDeviceAlbums,
            tracks: [],
            empty: EmptyShelf(
                icon: Icon.onDevice,
                title: "Nothing downloaded yet",
                detail: "Records you play or download from your server appear here, with how much of each is on this device."
            ),
            fractions: library.onDeviceFractions
        )
    }
}
