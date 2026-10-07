#if !os(tvOS)
import KoanFFI
import SwiftUI

/// What an imported EQ is for, asked once it is in: kōan cannot tell from a
/// file whether it corrects headphones or speakers, does that with a sound
/// added, or is taste for on top. Each answer says what it means; leaving it
/// for later keeps the profile a tuning, changed on its page.
struct RoleQuestion: View {
    let dsp: DspModel
    let names: [String]
    @Environment(\.dismiss) private var dismiss

    private var many: Bool { names.count > 1 }

    var body: some View {
        NavigationStack {
            KoanForm {
                Section {
                    Text(many
                         ? "kōan can't tell from the files what these EQs do."
                         : "kōan can't tell from the file what \(names.first ?? "this EQ") does.")
                        .koanText(.body)
                }
                Section {
                    choice(.correction,
                           "A neutral correction",
                           "Makes headphones or speakers sound neutral: an AutoEQ result, or a correction from a measurement such as spinorama's or REW's.")
                    choice(.baked,
                           "A correction with a sound in it",
                           "Corrects and adds taste in one, as presets named for a sound do, like Qudelix's “Lush”.")
                    choice(.tuning,
                           "A tuning",
                           "Taste on top of a correction: more bass, a darker treble.")
                }
                Section {
                    Button("Decide Later") { dismiss() }
                        .koanButton(.text)
                } footer: {
                    Text("Decided later, \(many ? "they stay tunings" : "it stays a tuning"), changed on \(many ? "each one's" : "its") page.")
                        .koanText(.fine, .muted)
                }
            }
            .navigationTitle(many ? "What Are These EQs?" : "What Is This EQ?")
        }
        #if os(macOS)
        .frame(minWidth: 460, minHeight: 420)
        #endif
    }

    private func choice(_ role: DspRole, _ title: String, _ example: String) -> some View {
        Button {
            dsp.setRole(names, role)
            dismiss()
        } label: {
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 8) {
                    Text(title).koanText(.body)
                    RoleTag(role: ProfileRole(role))
                }
                Text(example).koanText(.fine, .muted)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .contentShape(Rectangle())
        }
        .koanButton(.card)
    }
}

/// The profiles an import is asking about, as a sheet presents them.
struct RoleAsk: Identifiable {
    let names: [String]
    var id: String { names.joined(separator: "\u{0}") }
}
#endif
