import Foundation

/// Navigation values are distinct types, never raw IDs.
///
/// A `NavigationStack` matches destinations by type, so two `Int64` routes in
/// one stack collide silently and send you to whichever was registered first.
struct AlbumRoute: Hashable {
    let id: Int64
}

struct ArtistRoute: Hashable {
    let id: Int64
}
