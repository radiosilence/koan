#if !os(tvOS)
import KoanFFI
import SwiftUI

/// Several files chosen to import, shown before they are: whole presets
/// become a group, parts of one profile combine into one, and either is
/// named here.
private struct DspImportSheet: View {
    let dsp: DspModel
    let pending: PendingImport
    @Environment(\.dismiss) private var dismiss
    @State private var name: String

    init(dsp: DspModel, pending: PendingImport) {
        self.dsp = dsp
        self.pending = pending
        _name = State(initialValue: pending.plan.name)
    }

    private var plan: DspImportPlan { pending.plan }

    var body: some View {
        NavigationStack {
            KoanForm {
                Section {
                    ForEach(plan.files, id: \.self) { Text($0) }
                } header: {
                    Text(plan.group
                         ? "These \(plan.files.count) presets will become a group"
                         : "These \(plan.files.count) files will be combined into one EQ")
                } footer: {
                    Text(plan.group
                         ? "Each becomes an EQ of its own, and the group holds them. A group plays one at a time: the first, until you pick another on the group's page or in an output's preset menu."
                         : "They are parts of one setup, such as a response for each channel, so they play together as one EQ.")
                }
                Section(plan.group ? "Group name" : "Name") {
                    TextField("Name", text: $name).koanField()
                }
            }
            .navigationTitle(KoanTheme.label("Import"))
            .toolbar {
                KoanSheetAction(placement: .cancellationAction) {
                    Button("Cancel") {
                        dsp.pendingImport = nil
                        dismiss()
                    }
                }
                KoanSheetAction(placement: .confirmationAction) {
                    Button("Import") {
                        dsp.confirmImport(name: name.trimmingCharacters(in: .whitespaces))
                        dismiss()
                    }
                    .disabled(name.trimmingCharacters(in: .whitespaces).isEmpty)
                }
            }
        }
        #if os(macOS)
        .frame(minWidth: 440, minHeight: 360)
        #endif
    }
}

extension View {
    /// Confirm an import of several files, wherever one starts: Settings, or
    /// on iOS a share from another app.
    func dspImportConfirmation(_ dsp: DspModel) -> some View {
        sheet(item: Binding(get: { dsp.pendingImport }, set: { dsp.pendingImport = $0 })) { pending in
            DspImportSheet(dsp: dsp, pending: pending)
        }
    }
}
#endif
