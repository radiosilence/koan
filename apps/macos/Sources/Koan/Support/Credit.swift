import KoanFFI

/// A credit as the engine builds it: each credited artist linked where the
/// library knows them. Every artist the apps name comes as one of these;
/// nothing here builds, splits or falls back on a credit itself.
extension Array where Element == CreditPart {
    /// The credit as plain text, for what cannot hold a link: system
    /// now-playing metadata, accessibility labels, drag names.
    var text: String { map(\.text).joined() }
}
