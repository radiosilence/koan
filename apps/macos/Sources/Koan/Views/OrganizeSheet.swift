import AppKit
import KoanFFI
import SwiftUI

/// The TUI's organize modal, as a native sheet.
///
/// Most of this is the table. Choosing a pattern is two controls; being sure
/// about what it does to a hundred irreplaceable files is everything else, so
/// every selected file gets a row showing where it lands — and a file that
/// *can't* land keeps its row rather than being summarised into an error count
/// underneath. Nothing moves until the button is pressed.
struct OrganizeSheet: View {
    @Environment(OrganizeModel.self) private var organize
    @Environment(ActivityModel.self) private var activity

    var body: some View {
        @Bindable var organize = organize

        VStack(spacing: 0) {
            header
            Divider()
            controls
            Divider()
            table
            Divider()
            footer
        }
        // Flexible, so the window can be resized and the content follows.
        // `SheetChrome` is what gives it a starting size and a grip.
        .frame(
            minWidth: 560, maxWidth: .infinity,
            minHeight: 400, maxHeight: .infinity
        )
        .background(SheetChrome(initialSize: organize.size))
    }

    // MARK: - Header

    private var header: some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                Text("Organize Files")
                    .font(.headline)
                if let subject = organize.subject {
                    Text(subject.title)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
            Spacer()
            if organize.previewing {
                ProgressView().controlSize(.small)
            }
        }
        .padding(.horizontal, 18)
        .padding(.vertical, 13)
    }

    // MARK: - Pattern and destination

    @ViewBuilder
    private var controls: some View {
        @Bindable var organize = organize

        VStack(alignment: .leading, spacing: 9) {
            HStack(spacing: 12) {
                Picker("Pattern", selection: $organize.patternName) {
                    ForEach(organize.patterns, id: \.name) { pattern in
                        Text(pattern.name).tag(pattern.name as String?)
                    }
                }
                .frame(maxWidth: 240, alignment: .leading)
                .disabled(organize.editing)

                // Only worth asking when there is a choice. With one library
                // folder the answer is that folder, and the row below says so.
                if organize.folders.count > 1 {
                    Picker("Into", selection: $organize.baseDir) {
                        ForEach(organize.folders, id: \.self) { folder in
                            Text(shortFolder(folder)).tag(folder)
                        }
                    }
                    .frame(maxWidth: 240, alignment: .leading)
                }

                Spacer(minLength: 0)

                if organize.editing {
                    Button("Cancel") { organize.cancelEditing() }
                    Button("Save") { organize.saveEditing() }
                        .disabled(!organize.isModified)
                        .help("Store this pattern in config.toml under its name")
                } else {
                    Button("Edit") { organize.beginEditing() }
                        .disabled(organize.patternName == nil)
                }
            }

            if organize.editing {
                TextField("Format string", text: $organize.draft)
                    .textFieldStyle(.roundedBorder)
                    .font(.callout.monospaced())
            } else {
                Text(organize.pattern)
                    .font(.caption.monospaced())
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .textSelection(.enabled)
            }

            HStack(spacing: 6) {
                Toggle("Move cover art and cue sheets", isOn: $organize.moveAncillary)
                    .toggleStyle(.checkbox)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .help("Artwork, .cue and .log files in the same folder travel with the music")

                Text("·")

                // Destinations are relative to this, so it has to be visible
                // even when there was nothing to choose.
                Text("relative to \(organize.baseDir)")
                // An edited pattern previews and moves without being saved, so
                // say which state you are looking at.
                if organize.isModified {
                    Text("· unsaved changes")
                        .foregroundStyle(.orange)
                }
            }
            .font(.caption)
            .foregroundStyle(.tertiary)
        }
        .padding(.horizontal, 18)
        .padding(.vertical, 12)
    }

    private func shortFolder(_ path: String) -> String {
        URL(fileURLWithPath: path).lastPathComponent
    }

    // MARK: - The preview

    @ViewBuilder
    private var table: some View {
        if let error = organize.error {
            EmptyState(
                icon: "exclamationmark.triangle",
                title: "That pattern won't work",
                detail: error
            )
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else if let plan = organize.plan, !plan.entries.isEmpty {
            List(plan.entries, id: \.fromPath) { entry in
                OrganizeRow(entry: entry, baseDir: organize.baseDir)
                    .listRowSeparator(.hidden)
            }
            .listStyle(.inset)
        } else if organize.previewing {
            Color.clear
        } else {
            EmptyState(
                icon: "folder",
                title: organize.pattern.isEmpty ? "Choose a pattern" : "Nothing to organize",
                detail: organize.pattern.isEmpty
                    ? nil : "None of these tracks have a local file to move."
            )
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    // MARK: - Footer

    private var footer: some View {
        HStack(spacing: 12) {
            counts
            Spacer(minLength: 0)
            Button("Close") { organize.dismiss() }
                .keyboardShortcut(.cancelAction)
            Button(runTitle) { organize.run() }
                .keyboardShortcut(.defaultAction)
                .disabled(!canRun)
        }
        .padding(.horizontal, 18)
        .padding(.vertical, 13)
    }

    @ViewBuilder
    private var counts: some View {
        if let plan = organize.plan {
            HStack(spacing: 10) {
                Text(Format.count(Int64(plan.movedCount), "file") + " to move")
                if plan.unchangedCount > 0 {
                    Label("\(plan.unchangedCount) already in place", systemImage: "checkmark")
                        .foregroundStyle(.secondary)
                }
                if plan.conflictCount > 0 {
                    Label("\(plan.conflictCount) blocked", systemImage: "exclamationmark.triangle")
                        .foregroundStyle(.orange)
                }
                if plan.errorCount > 0 {
                    Label("\(plan.errorCount) failed", systemImage: "xmark.octagon")
                        .foregroundStyle(.red)
                }
                if plan.unresolved > 0 {
                    Label("\(plan.unresolved) not on disk", systemImage: "cloud")
                        .foregroundStyle(.secondary)
                }
            }
            .font(.caption)
            .labelStyle(.titleAndIcon)
        }
    }

    private var runTitle: String {
        if organize.running { return "Moving…" }
        guard let plan = organize.plan, plan.movedCount > 0 else { return "Move Files" }
        return "Move \(Format.count(Int64(plan.movedCount), "File"))"
    }

    /// Armed only when pressing it will actually move something: a plan with
    /// moves in it, nothing already in flight, and no other library task
    /// holding the database writer.
    private var canRun: Bool {
        !organize.running
            && !organize.previewing
            && !activity.isLibraryBusy
            && (organize.plan?.movedCount ?? 0) > 0
    }
}

/// One file: where it is, and where the pattern puts it.
///
/// The source is dimmed and the destination is not, because the destination is
/// the thing being decided. A row that isn't moving says why on the line where
/// the destination would have been — the whole point of showing it is that the
/// user sees the collision before the button, not after it.
private struct OrganizeRow: View {
    let entry: OrganizeEntry
    let baseDir: String

    var body: some View {
        HStack(alignment: .top, spacing: 9) {
            Image(systemName: icon)
                .foregroundStyle(tint)
                .font(.caption)
                .frame(width: 14)
                .padding(.top, 2)

            VStack(alignment: .leading, spacing: 2) {
                Text(entry.fromPath)
                    .font(.caption.monospaced())
                    .foregroundStyle(.tertiary)
                    .lineLimit(1)
                    .truncationMode(.middle)

                if let destination {
                    Text(destination)
                        .font(.callout.monospaced())
                        .foregroundStyle(tint)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }

                if let reason = entry.reason {
                    Text(reason)
                        .font(.caption)
                        .foregroundStyle(tint)
                }

                if !entry.ancillary.isEmpty {
                    // Named, not counted. Artwork and cue sheets moving with
                    // the music is usually wanted and occasionally not, and
                    // "+1 file" cannot tell you which.
                    Text("+ \(entry.ancillary.joined(separator: ", "))")
                        .font(.caption2)
                        .foregroundStyle(.tertiary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .help(entry.ancillary.joined(separator: "\n"))
                }
            }
        }
        .padding(.vertical, 3)
    }

    /// Relative to the library folder, which is the part the pattern decides.
    /// The absolute prefix is the same on every row and says nothing.
    private var destination: String? {
        guard let to = entry.toPath else { return nil }
        let prefix = baseDir.hasSuffix("/") ? baseDir : baseDir + "/"
        return to.hasPrefix(prefix) ? String(to.dropFirst(prefix.count)) : to
    }

    private var icon: String {
        switch entry.outcome {
        case .move: "arrow.right"
        case .unchanged: "checkmark"
        case .conflict: "exclamationmark.triangle.fill"
        case .error: "xmark.octagon.fill"
        }
    }

    private var tint: Color {
        switch entry.outcome {
        case .move: .primary
        case .unchanged: .secondary
        case .conflict: .orange
        case .error: .red
        }
    }
}

/// Gives the sheet a starting size and a resize grip.
///
/// Neither comes from SwiftUI. AppKit leaves `.resizable` off a sheet's style
/// mask, and SwiftUI pins the window's content size to whatever the content
/// fits — so the mask alone produces a window that calls itself resizable and
/// refuses to move. Both limits have to be lifted with it.
///
/// The starting size is passed in rather than measured here. Asking for the
/// parent window during presentation does not work: `sheetParent` is not set
/// until the sheet has begun, and `NSApp.mainWindow` is unreliable at that
/// moment. `OrganizeModel` works it out before the sheet exists, when the
/// window is unambiguous.
private struct SheetChrome: NSViewRepresentable {
    let initialSize: CGSize

    func makeNSView(context: Context) -> NSView {
        ChromeView(initialSize: initialSize)
    }

    func updateNSView(_ view: NSView, context: Context) {}

    private final class ChromeView: NSView {
        private let initialSize: CGSize
        /// Sized once. Re-applying on a later window change would yank it back
        /// under the user's own drag.
        private var sized = false

        init(initialSize: CGSize) {
            self.initialSize = initialSize
            super.init(frame: .zero)
        }

        @available(*, unavailable)
        required init?(coder: NSCoder) { fatalError("not from a nib") }

        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            guard !sized, let window else { return }
            sized = true
            allowResizing(window)
            window.setContentSize(NSSize(width: initialSize.width, height: initialSize.height))
        }

        /// SwiftUI re-applies its own sizing on later layout passes, which puts
        /// the limits back. Cheap to check, and self-healing if it does.
        override func layout() {
            super.layout()
            guard let window else { return }
            if !window.styleMask.contains(.resizable) || window.contentMaxSize.width < 10_000 {
                allowResizing(window)
            }
        }

        private func allowResizing(_ window: NSWindow) {
            window.styleMask.insert(.resizable)
            window.contentMinSize = NSSize(width: 560, height: 400)
            window.contentMaxSize = NSSize(width: 100_000, height: 100_000)
            window.minSize = NSSize(width: 560, height: 400)
            window.maxSize = NSSize(width: 100_000, height: 100_000)
        }
    }
}
