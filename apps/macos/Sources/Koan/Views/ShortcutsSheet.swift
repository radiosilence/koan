import SwiftUI

/// What `?` and ⌘/ show.
///
/// Two tables, because there are two kinds of key. Single-key shortcuts cannot
/// appear in the menu bar — that is the trade for them not stealing keys from
/// text fields — so this is the only place they are written down. The ⌘ ones
/// are in the menus, but nobody opens six menus to find out what a key does,
/// so they are here too. Both halves are generated from the tables that
/// implement them.
struct ShortcutsSheet: View {
    let hotkeys: [Hotkey]

    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            Text("Keyboard Shortcuts")
                .font(.title3.weight(.semibold))

            ScrollView {
                VStack(alignment: .leading, spacing: 20) {
                    columns { group in
                        hotkeys.filter { $0.group == group }
                            .map { Row(keys: $0.keys.map(Hotkey.caption), label: $0.label) }
                    }

                    Divider()

                    Text("With ⌘")
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(.secondary)

                    columns { group in
                        MenuShortcut.all.filter { $0.group == group }
                            .map { Row(keys: [$0.caption], label: $0.title) }
                    }
                }
            }
            .frame(maxHeight: 460)

            Text("None of these fire while you're typing.")
                .font(.caption)
                .foregroundStyle(.secondary)

            HStack {
                Spacer()
                Button("Done") { dismiss() }
                    .keyboardShortcut(.defaultAction)
            }
        }
        .padding(24)
        .frame(minWidth: 620)
    }

    /// One entry, whichever table it came from.
    private struct Row: Identifiable {
        let keys: [String]
        let label: String
        var id: String { label }
    }

    /// The groups side by side, skipping the ones this table has nothing in.
    private func columns(_ rows: @escaping (Hotkey.Group) -> [Row]) -> some View {
        HStack(alignment: .top, spacing: 30) {
            ForEach(Hotkey.Group.allCases, id: \.self) { group in
                let entries = rows(group)
                if !entries.isEmpty {
                    VStack(alignment: .leading, spacing: 7) {
                        Text(group.rawValue)
                            .font(.caption.weight(.semibold))
                            .foregroundStyle(.secondary)
                        ForEach(entries) { row(keys: $0.keys, label: $0.label) }
                    }
                }
            }
            Spacer(minLength: 0)
        }
    }

    private func row(keys: [String], label: String) -> some View {
        HStack(spacing: 9) {
            HStack(spacing: 3) {
                ForEach(keys, id: \.self) { key in
                    Text(key)
                        .font(.caption.monospaced())
                        .padding(.horizontal, 6)
                        .padding(.vertical, 3)
                        .glass(.regular, fallback: .quaternary, in: .rect(cornerRadius: 6))
                }
            }
            Text(label)
                .font(.callout)
            Spacer(minLength: 0)
        }
    }
}
