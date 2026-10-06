import KoanFFI
import SwiftUI

/// The DSP profiles a device can play through, and the one it does. Nil when
/// there are no profiles to choose from, so places without a choice say
/// nothing.
struct Presets {
    let current: String?
    let profiles: [String]
    /// What a device with no profile is said to play: "Off" for one of this
    /// device's own, "Original file" for a renderer.
    let none: String
    let enabled: Bool
    let choose: (String?) -> Void
    /// Turning processing on, where this device can: nil for another device's
    /// outputs, which are turned on there.
    let enable: (() -> Void)?

    @MainActor
    init?(dsp: DspModel, device: String, none: String) {
        guard let overview = dsp.overview, !overview.profiles.isEmpty else { return nil }
        current = dsp.profile(for: device)
        profiles = overview.profiles.map(\.name)
        self.none = none
        enabled = overview.enabled
        choose = { dsp.assign($0, to: device) }
        enable = { dsp.setEnabled(true) }
    }

    /// An output of the device in view, from what that device published.
    @MainActor
    init?(output: OutputInfo, of outputs: OutputsInfo, none: String, player: PlayerModel, dsp: DspModel) {
        guard !outputs.profiles.isEmpty else { return nil }
        current = output.preset
        profiles = outputs.profiles
        self.none = none
        enabled = outputs.dspEnabled
        choose = { player.setOutputPreset(device: output.id, profile: $0) }
        enable = outputs.owner == nil ? { dsp.setEnabled(true) } : nil
    }

    var summary: String {
        guard let current else { return none }
        return enabled ? current : "\(current), processing off"
    }
}

/// A device's presets as a menu: the profiles and the choice of none, with a
/// tick on the one it plays through. While processing is off everywhere, the
/// menu says so and offers to turn it on, so a choice is never one that
/// silently does nothing.
struct PresetMenu<Label: View>: View {
    let presets: Presets
    /// What the menu is for, such as the route a phone is playing to.
    var title: String?
    @ViewBuilder let label: () -> Label

    var body: some View {
        Menu {
            if !presets.enabled {
                Section {
                    if let enable = presets.enable {
                        Button("Turn On Processing", action: enable)
                    } else {
                        Text("Turn it on in that device's Settings")
                    }
                } header: {
                    KoanSectionHeader("Processing is off")
                }
            }
            if let title {
                Section(title) { picker }
            } else {
                picker
            }
        } label: {
            label()
        }.koanControl()
        .accessibilityLabel("Preset: \(presets.summary)")
    }

    private var picker: some View {
        Picker("Preset", selection: Binding(
            get: { presets.current ?? "" },
            set: { presets.choose($0.isEmpty ? nil : $0) }
        )) {
            Text(presets.none).tag("")
            Divider()
            ForEach(presets.profiles, id: \.self) { Text($0).tag($0) }
        }.koanControl()
        .pickerStyle(.inline)
    }
}
