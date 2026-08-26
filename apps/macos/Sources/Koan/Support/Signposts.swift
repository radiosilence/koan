import OSLog

/// Named regions for Instruments.
///
/// Signposts rather than logging: they cost a few nanoseconds when nothing is
/// recording, they nest, and they line up in a trace against what the CPU
/// profiler and the SwiftUI instrument saw at the same moment — which is the
/// only way to tell "our query was slow" from "SwiftUI spent that rebuilding
/// the page".
///
/// Record with:
/// `xcrun xctrace record --template 'SwiftUI' --attach koan-app --output t.trace`
enum Trace {
    /// `PointsOfInterest` specifically: it is the category Instruments'
    /// Points of Interest instrument captures, so regions named here line up on
    /// the same timeline as the CPU profiler's samples.
    static let signposter = OSSignposter(
        subsystem: "cc.blit.koan",
        category: OSLog.Category.pointsOfInterest.rawValue
    )

    /// Time a region. Returns whatever the body returns.
    static func region<T>(_ name: StaticString, _ body: () throws -> T) rethrows -> T {
        let id = signposter.makeSignpostID()
        let state = signposter.beginInterval(name, id: id)
        defer { signposter.endInterval(name, state) }
        return try body()
    }

    /// `isolation:` so the region inherits the caller's actor rather than
    /// hopping — otherwise wrapping a call in one changes what it measures.
    static func region<T>(
        _ name: StaticString,
        isolation: isolated (any Actor)? = #isolation,
        _ body: () async throws -> T
    ) async rethrows -> T {
        let id = signposter.makeSignpostID()
        let state = signposter.beginInterval(name, id: id)
        defer { signposter.endInterval(name, state) }
        return try await body()
    }
}
