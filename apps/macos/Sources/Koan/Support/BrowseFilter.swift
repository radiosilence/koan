import KoanFFI

extension BrowseFilter {
    static let none = BrowseFilter(
        favourites: false, lossless: false, codec: nil, yearFrom: nil, yearTo: nil, genre: nil
    )

    /// How many filters are on, the year range counting as one — what the
    /// control shows, because a narrowed grid otherwise looks like a missing
    /// library.
    var activeCount: Int {
        [favourites, lossless, codec != nil, genre != nil, yearFrom != nil || yearTo != nil]
            .filter { $0 }.count
    }

    /// As `UserDefaults` keeps it: a dictionary of what is set.
    var stored: [String: Any] {
        var out: [String: Any] = ["favourites": favourites, "lossless": lossless]
        out["codec"] = codec
        out["genre"] = genre
        out["yearFrom"] = yearFrom
        out["yearTo"] = yearTo
        return out
    }

    init(stored: [String: Any]) {
        self.init(
            favourites: stored["favourites"] as? Bool ?? false,
            lossless: stored["lossless"] as? Bool ?? false,
            codec: stored["codec"] as? String,
            yearFrom: (stored["yearFrom"] as? Int).map(Int32.init),
            yearTo: (stored["yearTo"] as? Int).map(Int32.init),
            genre: stored["genre"] as? String
        )
    }
}
