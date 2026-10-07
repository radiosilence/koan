#if !os(tvOS)
import KoanFFI
import SwiftUI

/// What an imported EQ is for, asked once it is in: kōan cannot tell from a
/// file whether it corrects headphones or speakers, does that with a sound
/// added, or is taste for on top. Each answer says what it means; leaving it
/// for later keeps the profile a tuning, changed on its page. An import
/// started from a device's chain goes into it as the answer says.
struct RoleQuestion: View {
    let dsp: DspModel
    let ask: RoleAsk
    @Environment(\.dismiss) private var dismiss

    private var many: Bool { ask.names.count > 1 }

    var body: some View {
        NavigationStack {
            KoanForm {
                Section {
                    Text(many
                         ? "kōan can't tell from the files what these EQs do."
                         : "kōan can't tell from the file what \(ask.names.first ?? "this EQ") does.")
                        .koanText(.body)
                } footer: {
                    if let into = ask.into {
                        Text(into.stage == .eq
                             ? "A tuning is added to \(dsp.label(into.device))'s tuning; a correction becomes its correction."
                             : "A correction becomes \(dsp.label(into.device))'s correction.")
                            .koanText(.fine, .muted)
                    }
                }
                Section {
                    choice(.correction,
                           "Makes headphones or speakers sound neutral: an AutoEQ result, or a correction from a measurement such as spinorama's or REW's.")
                    choice(.baked,
                           "Corrects and adds taste in one, as presets named for a sound do, like Qudelix's “Lush”.")
                    choice(.tuning,
                           "Taste on top of a correction: more bass, a darker treble.")
                }
                Section {
                    Button("Decide Later") {
                        dsp.answer(ask, nil)
                        dismiss()
                    }
                        .koanButton(.link)
                } footer: {
                    Text(many
                         ? "Decide later: they're kept as tunings, and you can change each one's role on its page."
                         : "Decide later: it's kept as a tuning, and you can change its role on its page.")
                        .koanText(.fine, .muted)
                }
            }
            .navigationTitle(KoanTheme.label(many ? "What are these EQs?" : "What is this EQ?"))
        }
        #if os(macOS)
        .frame(minWidth: 460, minHeight: 420)
        #endif
    }

    private func choice(_ role: DspRole, _ example: String) -> some View {
        Button {
            dsp.answer(ask, role)
            dismiss()
        } label: {
            VStack(alignment: .leading, spacing: 4) {
                Text(KoanTheme.label(ProfileRole(role).label)).koanText(.body)
                Text(example).koanText(.fine, .muted)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .contentShape(Rectangle())
        }
        .koanButton(.card)
    }
}
#endif
