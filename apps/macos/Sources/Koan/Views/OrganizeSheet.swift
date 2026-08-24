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
        // Fills whatever the sheet window is set to, rather than pinning a
        // size. `SheetChrome` is what sets that — see below.
        .frame(
            minWidth: 560, maxWidth: .infinity,
            minHeight: 400, maxHeight: .infinity
        )
        .background(SheetChrome(widthFraction: 0.9, heightFraction: 0.85))
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
                Picker("Pattern", selection: patternSelection) {
                    ForEach(organize.patterns, id: \.name) { pattern in
                        Text(pattern.name).tag(pattern.name as String?)
                    }
                    if !organize.patterns.isEmpty {
                        Divider()
                    }
                    Text("Custom…").tag(String?.none)
                }
                .frame(maxWidth: 260)

                // Only worth asking when there is a choice. With one library
                // folder the answer is that folder, and the row below says so.
                if organize.folders.count > 1 {
                    Picker("Into", selection: $organize.baseDir) {
                        ForEach(organize.folders, id: \.self) { folder in
                            Text(shortFolder(folder)).tag(folder)
                        }
                    }
                    .frame(maxWidth: 260)
                }

                Spacer(minLength: 0)
            }

            if organize.isCustom {
                TextField("Format string", text: $organize.customPattern)
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

            // Destinations are relative to this, so it has to be visible even
            // when there was nothing to choose.
            Text("Destinations are relative to \(organize.baseDir)")
                .font(.caption)
                .foregroundStyle(.tertiary)
        }
        .padding(.horizontal, 18)
        .padding(.vertical, 12)
    }

    /// Selecting "Custom…" seeds the field from whatever was showing, rather
    /// than handing over a blank one.
    private var patternSelection: Binding<String?> {
        Binding(
            get: { organize.patternName },
            set: { name in
                if let name {
                    organize.patternName = name
                } else {
                    organize.startCustomPattern()
                }
            }
        )
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
            Button(organize.outcome == nil ? "Cancel" : "Close") { organize.dismiss() }
                .keyboardShortcut(.cancelAction)
            // Gone once the run is done, rather than sitting there re-armed
            // over a plan that has already happened. Changing the pattern or
            // the destination brings it back.
            if organize.outcome == nil {
                Button(runTitle) { organize.run() }
                    .keyboardShortcut(.defaultAction)
                    .disabled(!canRun)
            }
        }
        .padding(.horizontal, 18)
        .padding(.vertical, 13)
    }

    @ViewBuilder
    private var counts: some View {
        if let outcome = organize.outcome {
            Text(outcome)
                .font(.callout)
        } else if let plan = organize.plan {
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

                if entry.ancillaryCount > 0 {
                    Text("+ \(Format.count(Int64(entry.ancillaryCount), "extra file")) alongside")
                        .font(.caption2)
                        .foregroundStyle(.tertiary)
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


/// Makes the sheet resizable and opens it large.
///
/// Neither is available from SwiftUI on macOS 14. AppKit leaves `.resizable`
/// off a sheet's style mask, so there is no grip however the content is framed;
/// and a sheet sizes itself to its content, so a flexible frame with nothing
/// driving it collapses toward the minimum instead of filling anything.
/// `.presentationSizing` solves the second on macOS 15, which is past our floor.
///
/// So both are asked for directly: insert the style mask, and take a starting
/// size from the window the sheet is attached to. The content's frame is
/// flexible, so it follows the window from then on — including the user's own
/// resizing, which is the point.
private struct SheetChrome: NSViewRepresentable {
    let widthFraction: CGFloat
    let heightFraction: CGFloat

    func makeNSView(context: Context) -> NSView {
        ChromeView(widthFraction: widthFraction, heightFraction: heightFraction)
    }

    func updateNSView(_ view: NSView, context: Context) {}

    private final class ChromeView: NSView {
        let widthFraction: CGFloat
        let heightFraction: CGFloat
        /// The sheet is configured once. Re-applying on a later window change
        /// would yank it back to the default size under the user's drag.
        private var configured = false

        init(widthFraction: CGFloat, heightFraction: CGFloat) {
            self.widthFraction = widthFraction
            self.heightFraction = heightFraction
            super.init(frame: .zero)
        }

        @available(*, unavailable)
        required init?(coder: NSCoder) { fatalError("not from a nib") }

        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            guard !configured, window != nil else { return }
            // `sheetParent` is only set once the sheet has actually begun, which
            // is after the view lands in it.
            DispatchQueue.main.async { [weak self] in self?.configure() }
        }

        private func configure() {
            guard !configured, let window else { return }
            configured = true
            window.styleMask.insert(.resizable)
            guard let parent = window.sheetParent else { return }
            window.setContentSize(
                NSSize(
                    width: parent.frame.width * widthFraction,
                    height: parent.frame.height * heightFraction
                )
            )
        }
    }
}
