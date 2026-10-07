import KoanFFI
import SwiftUI

/// An output's EQ as the sentence it is: these headphones, corrected to a
/// target, tuned with a tuning. Each part is its own choice, and kōan builds
/// the chain from them, adjusting the tuning to the correction's target. A
/// correction and its target are a whole setup; a tuning is offered, quietly,
/// on top.
struct OutputEqSection: View {
    let dsp: DspModel
    let overview: DspOverview
    let device: String
    /// Take a baked EQ apart, presented by the page.
    var split: ((String) -> Void)?
    @State private var targets: DspTargets?

    private var correction: DspProfileSummary? {
        overview.profiles.first { $0.name == overview.active }
    }

    private var corrections: [DspProfileSummary] {
        overview.profiles.filter { $0.role != .tuning }
    }

    /// Tunings are EQ: a profile with impulse responses corrects a room.
    private var tunings: [DspProfileSummary] {
        overview.profiles.filter { $0.role == .tuning && $0.rates.isEmpty }
    }

    private var tuning: DspProfileSummary? {
        overview.profiles.first { $0.name == overview.tuning }
    }

    /// The target the correction aims at, as chosen or as made for.
    private var aim: DspTargetOption? {
        guard let targets else { return nil }
        let id = targets.chosen ?? targets.madeFor?.id
        return targets.choices.first { $0.id == id }
    }

    var body: some View {
        Section {
            Picker("Correction", selection: Binding(
                get: { overview.active ?? "" },
                set: { dsp.assign($0.isEmpty ? nil : $0, to: device) }
            )) {
                Text("None").tag("")
                ForEach(corrections, id: \.name) { p in
                    Text(p.name).tag(p.name)
                }
            }
            .koanControl()
            .task(id: "\(overview.active ?? "")\u{0}\(dsp.stamp)") {
                targets = if let name = overview.active { await dsp.targets(name) } else { nil }
            }

            if let name = overview.active, let targets, correction?.role == .correction {
                Picker("Target", selection: Binding(
                    get: { targets.chosen ?? targets.madeFor?.id ?? "" },
                    set: { id in dsp.chooseTarget(name, id == targets.madeFor?.id ? nil : id) }
                )) {
                    ForEach(targets.choices, id: \.id) { c in
                        TargetRow(target: c, isDefault: c.id == targets.madeFor?.id).tag(c.id)
                    }
                }
                .koanControl()
                #if os(iOS)
                .pickerStyle(.navigationLink)
                #endif
            }

            if correction?.role == .baked, let name = overview.active {
                // Greyed with its reason, never silently missing.
                #if os(tvOS)
                Label("\(name) already has a tuning baked in. Split it on your phone or Mac to swap tunings.", systemImage: "info.circle")
                    .koanText(.meta, .muted)
                #else
                Label("\(name) already has a tuning baked in. Split it to swap tunings.", systemImage: "info.circle")
                    .koanText(.meta, .muted)
                // One EQ splits: not a stack or a group, nor responses.
                if let split, let c = correction, c.rates.isEmpty, c.layers == 0 {
                    Button("Split into Correction + Tuning…") { split(name) }
                }
                #endif
            } else if let tuning {
                Picker("Tuning", selection: Binding(
                    get: { tuning.name },
                    set: { dsp.setTuning($0.isEmpty ? nil : $0, for: device) }
                )) {
                    Text("None").tag("")
                    ForEach(tunings, id: \.name) { t in
                        Text(t.name).tag(t.name)
                    }
                }
                .koanControl()
                // A group of tunings is a quick switch between them.
                if !tuning.members.isEmpty {
                    Picker("Playing", selection: Binding(
                        get: { tuning.playing ?? tuning.members.first ?? "" },
                        set: { dsp.select(tuning.name, $0) }
                    )) {
                        ForEach(tuning.members, id: \.self) { Text($0).tag($0) }
                    }
                    .koanControl()
                }
            } else if !tunings.isEmpty {
                Menu("Add a Tuning…") {
                    ForEach(tunings, id: \.name) { t in
                        Button(t.name) { dsp.setTuning(t.name, for: device) }
                    }
                }
                .koanControl()
            }
            // A choice that does not play says so, and why.
            if let leftOut = overview.leftOut {
                Label(leftOut, systemImage: "exclamationmark.triangle")
                    .koanText(.meta, .bad)
            }
        } header: {
            Text(dsp.label(device)).koanText(.fine, .ink).textCase(nil)
        } footer: {
            Text(sentence)
                .koanText(.fine, .muted)
        }
        .disabled(!overview.enabled)
    }

    /// The chain in words: what plays, and what could.
    private var sentence: String {
        guard let name = overview.active else {
            return "Pick a correction for your headphones or speakers to make them neutral first."
        }
        var parts = [name]
        if let aim { parts.append("corrected to \(aim.name)") }
        if let tuning, overview.tuningPlays { parts.append("tuned with \(tuning.name)") }
        var text = parts.joined(separator: ", ") + "."
        if tuning == nil, correction?.role == .correction {
            text += " Optional: add a tuning (your taste) on top."
        }
        return text
    }
}
