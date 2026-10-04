import UIKit
import UniformTypeIdentifiers

/// "Share → kōan" for EQ and filter files, and text: a zip sent in a chat, an
/// AutoEQ file from Files, EQ lines pasted into a message.
///
/// An extension gets a sliver of memory and no engine, so it does not import
/// anything itself. What it is handed goes into the app group's inbox, one
/// folder per share, and the app imports it the next time it comes to the
/// front. An extension may not open its app, so it says to.
final class ShareViewController: UIViewController {
    static let group = "group.cc.blit.koan"

    private let label = UILabel()

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .systemBackground
        label.text = "Saving…"
        label.font = .preferredFont(forTextStyle: .headline)
        label.numberOfLines = 0
        label.textAlignment = .center
        label.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(label)
        NSLayoutConstraint.activate([
            label.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            label.centerYAnchor.constraint(equalTo: view.centerYAnchor),
            label.leadingAnchor.constraint(greaterThanOrEqualTo: view.leadingAnchor, constant: 24),
        ])
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        Task { await take() }
    }

    private func take() async {
        let providers = (extensionContext?.inputItems as? [NSExtensionItem] ?? [])
            .flatMap { $0.attachments ?? [] }
        guard
            let inbox = FileManager.default
                .containerURL(forSecurityApplicationGroupIdentifier: Self.group)?
                .appending(path: "Inbox/\(UUID().uuidString)", directoryHint: .isDirectory),
            (try? FileManager.default.createDirectory(at: inbox, withIntermediateDirectories: true)) != nil
        else {
            return finish("kōan could not take this.")
        }

        var saved = 0
        for (n, provider) in providers.enumerated() {
            if await save(provider, into: inbox, index: n) { saved += 1 }
        }
        if saved == 0 {
            try? FileManager.default.removeItem(at: inbox)
            return finish("Nothing here kōan can import.")
        }
        finish("Saved. Open kōan to finish importing.")
    }

    /// A file is copied as it is; text with no file behind it is written to
    /// one, which the importer reads by what it says.
    private func save(_ provider: NSItemProvider, into inbox: URL, index: Int) async -> Bool {
        let types = provider.registeredContentTypes
        let onlyText = !types.isEmpty && types.allSatisfy { $0.conforms(to: .text) && !$0.conforms(to: .fileURL) }
        if onlyText, provider.canLoadObject(ofClass: String.self) {
            let text: String? = await withCheckedContinuation { done in
                _ = provider.loadObject(ofClass: String.self) { value, _ in done.resume(returning: value) }
            }
            guard let text, !text.isEmpty else { return false }
            let dest = inbox.appending(path: index == 0 ? "Shared.txt" : "Shared \(index).txt")
            return (try? text.write(to: dest, atomically: true, encoding: .utf8)) != nil
        }
        guard let type = types.first(where: { $0.conforms(to: .data) || $0.conforms(to: .directory) }) ?? types.first else {
            return false
        }
        return await withCheckedContinuation { done in
            _ = provider.loadFileRepresentation(for: type, openInPlace: false) { url, _, _ in
                // The file is only there for the length of this handler.
                guard let url else { return done.resume(returning: false) }
                let dest = inbox.appending(path: url.lastPathComponent)
                done.resume(returning: (try? FileManager.default.copyItem(at: url, to: dest)) != nil)
            }
        }
    }

    private func finish(_ message: String) {
        label.text = message
        Task {
            try? await Task.sleep(for: .seconds(1.5))
            extensionContext?.completeRequest(returningItems: nil)
        }
    }
}
