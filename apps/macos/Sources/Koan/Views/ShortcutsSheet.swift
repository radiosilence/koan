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
            Text("Keyboard Shortcuts").koanCase()
                .koanText(.titleSmall, .strong)

            ScrollView {
                VStack(alignment: .leading, spacing: 20) {
                    columns { group in
                        hotkeys.filter { $0.group == group }
                            .map { Row(keys: $0.keys.map(Hotkey.caption), label: $0.label) }
                    }

                    KoanDivider()

                    KoanSectionHeader("With ⌘")

                    columns { group in
                        MenuShortcut.all.filter { $0.group == group }
                            .map { Row(keys: [$0.caption], label: $0.title) }
                    }
                }
            }
            .frame(maxHeight: 460)

            Text("None of these fire while you're typing.")
                .koanText(.fine, .muted)

            HStack {
                Spacer()
                Button("Done") { dismiss() }
                    .keyboardShortcut(.defaultAction)
                    .koanButton(.standard)
            }
        }
        .padding(24)
        .frame(minWidth: 620)
        .koanSheet()
    }

    /// One entry, whichever table it came from.
    private struct Row: Identifiable {
        let keys: [String]
        let label: String
        var id: String { label }
    }

    /// The groups side by side, skipping the ones this table has nothing in,
    /// as many to a line as fit: a wider face takes fewer, and a label is
    /// never broken mid-word to squeeze in another.
    private func columns(_ rows: @escaping (Hotkey.Group) -> [Row]) -> some View {
        LazyVGrid(
            columns: [GridItem(.adaptive(minimum: 210), spacing: 30, alignment: .topLeading)],
            alignment: .leading,
            spacing: 20
        ) {
            ForEach(Hotkey.Group.allCases.filter { !rows($0).isEmpty }, id: \.self) { group in
                VStack(alignment: .leading, spacing: 7) {
                    KoanSectionHeader(group.rawValue)
                    ForEach(rows(group)) { row(keys: $0.keys, label: $0.label) }
                }
            }
        }
    }

    private func row(keys: [String], label: String) -> some View {
        HStack(spacing: 9) {
            HStack(spacing: 3) {
                ForEach(keys, id: \.self) { key in
                    Text(key)
                        .koanBadge()
                }
            }
            Text(label)
                .koanText(.meta)
            Spacer(minLength: 0)
        }
    }
}
