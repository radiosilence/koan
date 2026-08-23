import SwiftUI

/// What `?` shows.
///
/// Single-key shortcuts cannot appear in the menu bar — that is the trade for
/// them not stealing keys from text fields — so this is the only place they are
/// written down, and it is generated from the table that implements them.
struct ShortcutsSheet: View {
    let hotkeys: [Hotkey]

    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            Text("Keyboard Shortcuts")
                .font(.title3.weight(.semibold))

            HStack(alignment: .top, spacing: 34) {
                ForEach(Hotkey.Group.allCases, id: \.self) { group in
                    let rows = hotkeys.filter { $0.group == group }
                    if !rows.isEmpty {
                        VStack(alignment: .leading, spacing: 7) {
                            Text(group.rawValue)
                                .font(.caption.weight(.semibold))
                                .foregroundStyle(.secondary)
                            ForEach(rows, id: \.label) { hotkey in
                                row(hotkey)
                            }
                        }
                    }
                }
            }

            Text("Anything with ⌘ is in the menus. None of these fire while you're typing.")
                .font(.caption)
                .foregroundStyle(.secondary)

            HStack {
                Spacer()
                Button("Done") { dismiss() }
                    .keyboardShortcut(.defaultAction)
            }
        }
        .padding(24)
        .frame(minWidth: 560)
    }

    private func row(_ hotkey: Hotkey) -> some View {
        HStack(spacing: 9) {
            HStack(spacing: 3) {
                ForEach(hotkey.keys, id: \.self) { key in
                    Text(key == " " ? "space" : key)
                        .font(.caption.monospaced())
                        .padding(.horizontal, 5)
                        .padding(.vertical, 2)
                        .background(.quaternary, in: RoundedRectangle(cornerRadius: 4))
                }
            }
            Text(hotkey.label)
                .font(.callout)
            Spacer(minLength: 0)
        }
    }
}
