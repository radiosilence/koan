import Foundation
import KoanFFI

/// Dropped files, given library rows.
///
/// Shared by the queue and by playlists: both want the same thing out of a
/// drop — track ids — and both have to show something while it happens. Holds
/// the local library, because it reads tags and writes the rows a scan writes,
/// and a folder of a few hundred files takes long enough that a drop with no
/// sign of life reads as a drop that missed.
///
/// The files are indexed where they lie rather than copied anywhere. Rows are
/// the point: they are what let organize move the drop into the music tree
/// afterwards, on purpose and with a preview.
@MainActor
enum FileImport {
    /// The tracks the drop became, or nothing — which is reported here rather
    /// than left to each caller to phrase differently.
    static func trackIds(
        for urls: [URL],
        engine: KoanEngine,
        activity: ActivityModel?,
        report: (String) -> Void
    ) async -> [Int64] {
        let paths = urls.filter(\.isFileURL).map(\.path)
        guard !paths.isEmpty else { return [] }
        let summary = try? await activity?.run("Adding dropped files", uses: .localLibrary) {
            try await engine.importFiles(paths: paths)
        }.get()
        guard let summary, !summary.trackIds.isEmpty else {
            report("Nothing there koan can play.")
            return []
        }
        if let first = summary.errors.first {
            report(first)
        }
        return summary.trackIds
    }
}
