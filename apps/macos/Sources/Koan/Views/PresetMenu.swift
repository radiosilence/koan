import KoanFFI
import SwiftUI

/// The EQ presets a device can be set from, and which it was: a preset's
/// name, Flat, or Unsaved for EQ no preset holds. Nil when there is no EQ at
/// all to choose, so places without a choice say nothing.
struct Presets {
    /// The preset it was set from.
    let current: String?
    /// Changed since it was set from `current`.
    let edited: Bool
    /// No correction and no tuning: it plays untouched.
    let flat: Bool
    let presets: [String]
    /// Set it from a preset, or flat with nil.
    let choose: (String?) -> Void
    /// The device whose EQ page Edit… opens: only this device's own.
    let device: String?

    @MainActor
    init?(dsp: DspModel, device: String) {
        guard let overview = dsp.overview, !overview.profiles.isEmpty else { return nil }
        let state = overview.outputs.first { $0.device == device }
        current = state?.preset
        edited = state?.presetEdited ?? false
        flat = state?.flat ?? true
        presets = overview.profiles.filter(\.preset).map(\.name)
        choose = { dsp.applyPreset($0, to: device) }
        self.device = device
    }

    /// An output of the device in view, from what that device published.
    @MainActor
    init?(output: OutputInfo, of outputs: OutputsInfo, player: PlayerModel) {
        guard !outputs.profiles.isEmpty || output.preset != nil || output.unsaved else { return nil }
        current = output.preset
        edited = output.preset != nil && output.unsaved
        flat = output.preset == nil && !output.unsaved
        presets = outputs.profiles
        choose = { player.setOutputPreset(device: output.id, profile: $0) }
        device = outputs.owner == nil ? output.id : nil
    }

    /// What it is set to, in a word or two.
    var summary: String {
        if let current { return edited ? "\(current), edited" : current }
        return flat ? "Flat" : "Unsaved"
    }
}

/// A device's presets as a menu: Flat and each preset, a tick on the one it
/// was set from, and Edit… for the whole EQ page. A preset changed since is
/// listed twice: as edited, ticked, and as saved, which goes back to it.
struct PresetMenu<Label: View>: View {
    @Environment(AppState.self) private var app
    let presets: Presets
    /// What the menu is for, such as the route a phone is playing to.
    var title: String?
    @ViewBuilder let label: () -> Label
    #if os(macOS)
    @Environment(\.openSettings) private var openSettings
    #elseif os(iOS)
    @State private var editing = false
    #endif

    var body: some View {
        KoanMenu {
            if let title {
                Section(title) { choices }
            } else {
                choices
            }
            #if !os(tvOS)
            if let device = presets.device {
                Divider()
                Button("Edit…") { edit(device) }
            }
            #endif
        } label: {
            label()
        }
        .koanControl()
        .accessibilityLabel("Preset: \(presets.summary)")
        #if os(iOS)
        .sheet(isPresented: $editing) {
            NavigationStack {
                EqSettings(device: presets.device)
                    .navigationTitle(KoanTheme.label("EQ"))
                    .toolbar {
                        KoanSheetAction(placement: .confirmationAction) {
                            Button("Done") { editing = false }
                        }
                    }
            }
            .koanSheet()
        }
        #endif
    }

    private func edit(_ device: String) {
        #if os(macOS)
        app.dsp.editing = device
        openSettings()
        #elseif os(iOS)
        editing = true
        #endif
    }

    private static var unsaved: String { "\u{0}unsaved" }
    private static var edited: String { "\u{0}edited" }

    private var selected: String {
        guard let current = presets.current else { return presets.flat ? "" : Self.unsaved }
        return presets.edited ? Self.edited : current
    }

    private func select(_ tag: String) {
        guard tag != Self.unsaved, tag != Self.edited else { return }
        presets.choose(tag.isEmpty ? nil : tag)
    }

    /// Flat, Unsaved when it applies, then each preset, an edited one listed
    /// twice: as edited, and as saved.
    private var options: [(label: String, tag: String)] {
        var options = [(label: "Flat", tag: "")]
        if presets.current == nil, !presets.flat {
            options.append((label: "Unsaved", tag: Self.unsaved))
        }
        for name in presets.presets {
            if name == presets.current, presets.edited {
                options.append((label: "\(name) (\(KoanTheme.label("edited")))", tag: Self.edited))
            }
            options.append((label: name, tag: name))
        }
        return options
    }

    /// In the theme on the Mac, the options as the theme menu's rows with a
    /// tick on the chosen one; the system's inline picker otherwise.
    @ViewBuilder
    private var choices: some View {
        #if os(macOS)
        if KoanTheme.isOn {
            ForEach(options, id: \.tag) { option in
                KoanMenuChoice(option.label, chosen: option.tag == selected) { select(option.tag) }
            }
        } else {
            picker
        }
        #else
        picker
        #endif
    }

    private var picker: some View {
        Picker("Preset", selection: Binding(get: { selected }, set: select)) {
            Text("Flat").tag("")
            if presets.current == nil, !presets.flat {
                Text("Unsaved").tag(Self.unsaved)
            }
            if !presets.presets.isEmpty {
                Divider()
            }
            ForEach(presets.presets, id: \.self) { name in
                if name == presets.current, presets.edited {
                    Text("\(name) (\(KoanTheme.label("edited")))").tag(Self.edited)
                }
                Text(name).tag(name)
            }
        }
        .koanControl()
        .pickerStyle(.inline)
    }
}
