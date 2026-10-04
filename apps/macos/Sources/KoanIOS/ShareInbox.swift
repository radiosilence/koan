import Foundation

/// What the share extension left in the app group: one folder per share,
/// imported when the app comes to the front.
///
/// Each folder is moved into the app's own temporary directory before it is
/// imported, so a share is taken exactly once, and an import that stops to ask
/// for a sample rate still has its files when the answer comes.
@MainActor
enum ShareInbox {
    static let group = "group.cc.blit.koan"

    static func collect(into dsp: DspModel) {
        let files = FileManager.default
        guard
            let inbox = files
                .containerURL(forSecurityApplicationGroupIdentifier: group)?
                .appending(path: "Inbox", directoryHint: .isDirectory),
            let shares = try? files.contentsOfDirectory(at: inbox, includingPropertiesForKeys: nil)
        else { return }
        for share in shares {
            let taken = files.temporaryDirectory.appending(path: "shared-\(UUID().uuidString)")
            guard (try? files.moveItem(at: share, to: taken)) != nil,
                  let contents = try? files.contentsOfDirectory(at: taken, includingPropertiesForKeys: nil),
                  !contents.isEmpty
            else { continue }
            dsp.importFiles(contents.sorted { $0.path < $1.path })
        }
    }
}
