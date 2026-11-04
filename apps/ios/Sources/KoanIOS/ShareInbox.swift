import Foundation

/// What the share extension left in the app group: one folder per share,
/// imported when the app comes to the front.
///
/// Each folder is moved into the app's own temporary directory before it is
/// imported, so a share is taken exactly once, and an import that stops to ask
/// for a sample rate still has its files when the answer comes. The folder is
/// deleted once the import is done with it.
@MainActor
enum ShareInbox {
    static let group = "group.cc.blit.koan"

    /// Delete what iOS copied into the app's Inbox for Open-in imports that
    /// never finished: the app quit with one waiting to be confirmed. Only
    /// copies made well before this launch, so the file whose opening
    /// launched the app is still there to import.
    static func sweepOpened() {
        let files = FileManager.default
        guard let documents = files.urls(for: .documentDirectory, in: .userDomainMask).first,
              let opened = try? files.contentsOfDirectory(
                  at: documents.appending(path: "Inbox", directoryHint: .isDirectory),
                  includingPropertiesForKeys: [.creationDateKey]
              )
        else { return }
        let before = Date.now.addingTimeInterval(-10 * 60)
        for file in opened {
            let made = (try? file.resourceValues(forKeys: [.creationDateKey]))?.creationDate ?? .distantPast
            if made < before { try? files.removeItem(at: file) }
        }
    }

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
            else {
                try? files.removeItem(at: taken)
                continue
            }
            dsp.importFiles(contents.sorted { $0.path < $1.path }, copy: taken)
        }
    }
}
