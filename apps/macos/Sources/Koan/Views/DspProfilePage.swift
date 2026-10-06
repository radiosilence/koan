import KoanFFI
import SwiftUI

/// One profile, and exactly what is in it: each impulse response's rate,
/// channels, length and routing, any bands, the headroom it is given, and the
/// outputs that play through it. Renamed and deleted from here.
struct DspProfilePage: View {
    let dsp: DspModel
    @State var name: String
    @Environment(\.dismiss) private var dismiss

    @State private var detail: DspProfileDetail?
    @State private var response: DspResponse?
    @State private var targets: DspTargets?
    /// Every target, for what a ready-made EQ was made for.
    @State private var madeForChoices: [DspTargetOption] = []
    @State private var addingTarget = false
    @State private var editingName = ""
    @State private var confirmingDelete = false
    /// Bands, responses, headroom and sync, for a correction: most people
    /// pick a correction and its target and are done.
    @State private var showingMore = false

    var body: some View {
        Form {
            if let d = detail {
                Section {
                    ChainSummaryCard(corrects: d.corrects, baked: d.correctsBaked, tunings: d.tunings)
                    if let twice = d.correctsTwice {
                        Label(twice, systemImage: "exclamationmark.triangle.fill")
                            .foregroundStyle(.orange)
                    }
                }
            }
            Section {
                TextField("Name", text: $editingName)
                    .onSubmit(rename)
            }

            if let d = detail {
                if let r = response {
                    Section {
                        EqGraph(response: r, handles: BandTable.handles(d.bands)) { index, hz, db in
                            let b = d.bands[index]
                            dsp.setBand(name, index, kind: b.kind, freq: hz, gain: db, q: b.q)
                        }
                    }
                }
                if let problem = d.problem {
                    Section {
                        Label(problem, systemImage: "exclamationmark.triangle.fill")
                            .foregroundStyle(.orange)
                    }
                }

                Section("Used for") {
                    if d.devices.isEmpty {
                        Text("No output yet")
                            .foregroundStyle(.secondary)
                    }
                    ForEach(d.devices, id: \.self) { Text(dsp.label($0)) }
                    if let device = dsp.overview?.device {
                        if d.devices.contains(device) {
                            Button("Stop using for \(dsp.label(device))") { dsp.use(nil) }
                        } else {
                            Button("Use for \(dsp.label(device))") { dsp.use(d.name) }
                        }
                    }
                }

                RoleSection(dsp: dsp, detail: d, madeForChoices: madeForChoices,
                            targets: targets, adding: $addingTarget)
                if d.group {
                    GroupSection(dsp: dsp, detail: d)
                } else {
                    LayersSection(dsp: dsp, detail: d)
                }

                // A correction is finished as installed; what it is made of
                // is there for those who look. A tuning is its bands.
                if d.role != .tuning {
                    Section {
                        Button {
                            withAnimation { showingMore.toggle() }
                        } label: {
                            HStack {
                                Text(showingMore ? "Less" : "Bands, sync and more")
                                Spacer()
                                Image(systemName: "chevron.right")
                                    .rotationEffect(.degrees(showingMore ? 90 : 0))
                                    .foregroundStyle(.tertiary)
                            }
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                    }
                }
                if d.role == .tuning || showingMore {
                    more(d)
                }

                Section {
                    Button("Delete Profile", role: .destructive) { confirmingDelete = true }
                }
            } else {
                ProgressView()
            }
        }
        .formStyle(.grouped)
        .navigationTitle(name)
        .task(id: dsp.stamp) { await load() }
        #if !os(tvOS)
        .filePicker(
            isPresented: $addingTarget,
            allowedContentTypes: [.commaSeparatedText, .plainText, .text, .item],
            allowsMultipleSelection: false
        ) { result in
            if case let .success(urls) = result, let url = urls.first {
                Task { await dsp.addTarget(url) }
            }
        }
        #endif
        .confirmationDialog(
            "Delete \(name)?",
            isPresented: $confirmingDelete,
            titleVisibility: .visible
        ) {
            Button("Delete", role: .destructive) {
                dsp.remove(name)
                dismiss()
            }
        } message: {
            Text("Its impulse responses are deleted with it.")
        }
    }

    @ViewBuilder private func more(_ d: DspProfileDetail) -> some View {
        ScopeSection(dsp: dsp, detail: d)

        if !d.impulses.isEmpty {
            Section {
                ForEach(Array(d.impulses.enumerated()), id: \.offset) { _, ir in
                    ImpulseRow(ir: ir)
                }
            } header: {
                Text("Impulse responses")
            } footer: {
                Text("A track at a rate with no response of its own is resampled to the nearest one here.")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
            }
        }

        BandTable(dsp: dsp, profile: name, bands: d.bands)

        Section {
            LabeledContent("Preamp", value: "\(String(format: "%.1f", d.preampDb)) dB")
        } header: {
            Text("Headroom")
        } footer: {
            Text(d.preampSet
                 ? "Set in the profile."
                 : "Derived at \(DspModel.khz(d.preampRate)) kHz from the largest gain the filters apply, so nothing they boost can clip.")
                .font(.caption)
                .foregroundStyle(.tertiary)
        }

        if !d.source.isEmpty {
            Section("Imported from") {
                ForEach(d.source, id: \.self) { Text($0).foregroundStyle(.secondary) }
            }
        }
    }

    private func load() async {
        detail = await dsp.detail(name)
        response = await dsp.response(name)
        targets = await dsp.targets(name)
        if madeForChoices.isEmpty {
            madeForChoices = await dsp.targetsFor(inEar: false) + dsp.targetsFor(inEar: true)
        }
        editingName = name
    }

    private func rename() {
        let new = editingName.trimmingCharacters(in: .whitespaces)
        guard !new.isEmpty, new != name else {
            editingName = name
            return
        }
        Task {
            if await dsp.rename(name, to: new) {
                name = new
            } else {
                editingName = name
            }
        }
    }
}

/// A group's members, one playing, chosen as a radio button is.
private struct GroupSection: View {
    let dsp: DspModel
    let detail: DspProfileDetail

    var body: some View {
        Section {
            Picker("Playing", selection: Binding(
                get: { detail.layers.first(where: \.on)?.profile ?? detail.layers.first?.profile ?? "" },
                set: { dsp.select(detail.name, $0) }
            )) {
                ForEach(detail.layers, id: \.profile) { Text($0.profile).tag($0.profile) }
            }
            .pickerStyle(.inline)
            .labelsHidden()
            #if !os(tvOS)
            Button("Make It a Stack of Layers") { dsp.setGroup(detail.name, false) }
            #endif
        } header: {
            Text("Group: pick one")
        } footer: {
            Text(detail.layers.contains(where: \.on)
                 ? "One member plays at a time. Pick another and it plays in place of the last. Each member is a profile of its own, with its own page."
                 : "None was picked, so the first plays. Pick one to change it.")
                .font(.caption)
                .foregroundStyle(.tertiary)
        }
    }
}

/// The profiles a stack plays first, in order, each switched on or off: a
/// headphone's correction, then taste on top of it. Any profile can become a
/// stack; one with impulse responses cannot be a layer.
private struct LayersSection: View {
    let dsp: DspModel
    let detail: DspProfileDetail

    private var layers: [DspLayerInfo] { detail.layers }

    /// Profiles that could be added: not this one, not already in, and EQ
    /// alone.
    private var addable: [DspProfileSummary] {
        (dsp.overview?.profiles ?? []).filter { p in
            p.name != detail.name
                && p.rates.isEmpty
                && !layers.contains { $0.profile == p.name }
        }
    }

    /// Tunings first, since adding one is what most people come here for.
    private var addMenu: some View {
        Menu(layers.isEmpty ? "Add a Tuning…" : "Add a Layer") {
            ForEach(addable.filter { $0.role == .tuning }, id: \.name) { p in
                Button(p.name) { add(p) }
            }
            let others = addable.filter { $0.role != .tuning }
            if !others.isEmpty {
                Section("Corrections") {
                    ForEach(others, id: \.name) { p in
                        Button("\(p.name) · \(ProfileRole(p.role).label)") { add(p) }
                    }
                }
            }
        }
    }

    var body: some View {
        if layers.isEmpty {
            // Nothing on top: a quiet offer, not an empty section.
            if !addable.isEmpty {
                Section { addMenu }
            }
        } else {
            stack
        }
    }

    private var stack: some View {
        Section {
            ForEach(Array(layers.enumerated()), id: \.element.profile) { index, layer in
                Toggle(isOn: Binding(
                    get: { layer.on },
                    set: { on in
                        var changed = layers
                        changed[index].on = on
                        dsp.setLayers(detail.name, changed)
                    }
                )) {
                    HStack(spacing: 8) {
                        Text(layer.profile)
                        if index < detail.layerRoles.count, let role = detail.layerRoles[index] {
                            RoleTag(role: ProfileRole(role))
                        }
                    }
                }
                #if !os(tvOS)
                .contextMenu {
                    Button("Move Up") { move(index, by: -1) }
                        .disabled(index == 0)
                    Button("Move Down") { move(index, by: 1) }
                        .disabled(index == layers.count - 1)
                    Button("Remove from Stack", role: .destructive) { remove(index) }
                }
                #endif
            }
            #if os(iOS)
            .onMove { from, to in
                var changed = layers
                changed.move(fromOffsets: from, toOffset: to)
                dsp.setLayers(detail.name, changed)
            }
            .onDelete { offsets in
                var changed = layers
                changed.remove(atOffsets: offsets)
                dsp.setLayers(detail.name, changed)
            }
            #endif
            if !addable.isEmpty { addMenu }
            #if !os(tvOS)
            if layers.count > 1 {
                Button("Make It a Group, One Playing at a Time") { dsp.setGroup(detail.name, true) }
            }
            #endif
        } header: {
            Text("Layers")
        } footer: {
            Text("Played in order, before this profile's own filters: a correction, then tunings on top. A layer switched off plays nothing.")
                .font(.caption)
                .foregroundStyle(.tertiary)
        }
    }

    private func add(_ p: DspProfileSummary) {
        dsp.setLayers(detail.name, layers + [DspLayerInfo(profile: p.name, on: true)])
    }

    private func move(_ index: Int, by step: Int) {
        var changed = layers
        changed.swapAt(index, index + step)
        dsp.setLayers(detail.name, changed)
    }

    private func remove(_ index: Int) {
        var changed = layers
        changed.remove(at: index)
        dsp.setLayers(detail.name, changed)
    }
}

/// What a profile is for, in a line each, always at the top of its page:
/// the headphone the chain corrects and how, and the tuning on top. Each in
/// its role's colour, as the layers and the graph show them.
struct ChainSummaryCard: View {
    let corrects: String?
    var baked = false
    let tunings: [String]

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            if let corrects {
                RoleLine(role: baked ? .baked : .correction, text: corrects)
            } else {
                RoleLine(role: .correction, text: "None", muted: true)
            }
            if !tunings.isEmpty {
                RoleLine(role: .tuning, text: tunings.joined(separator: ", "))
            }
        }
    }
}

/// The three things a profile can be for, each with its label and colour,
/// the same wherever profiles are listed or drawn.
enum ProfileRole {
    case correction, tuning, baked

    init(_ role: DspRole) {
        switch role {
        case .correction: self = .correction
        case .tuning: self = .tuning
        case .baked: self = .baked
        }
    }

    var label: String {
        switch self {
        case .correction: "Correction"
        case .tuning: "Tuning"
        case .baked: "Baked"
        }
    }

    var color: Color {
        switch self {
        case .correction: .koanAccent
        case .tuning: .orange
        case .baked: .purple
        }
    }
}

private struct RoleLine: View {
    let role: ProfileRole
    let text: String
    var muted = false

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            RoleTag(role: role)
            Text(text)
                .foregroundStyle(muted ? .secondary : .primary)
        }
    }
}

/// A role's badge, in its colour: beside a layer, in a list of profiles,
/// in the summary.
struct RoleTag: View {
    let role: ProfileRole

    var body: some View {
        Text(role.label)
            .font(.caption2.weight(.semibold))
            .foregroundStyle(role.color)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(role.color.opacity(0.15), in: Capsule())
    }
}

extension DspTargetOption {
    /// Its name, and what it does in a few plain words.
    var label: String { does.isEmpty ? name : "\(name): \(does)" }
}

/// A target in a picker: on a phone, its name with what it does beneath, in
/// a list of its own; on the Mac, both in the menu's one line.
struct TargetRow: View {
    let target: DspTargetOption

    var body: some View {
        #if os(macOS)
        Text(target.label)
        #else
        VStack(alignment: .leading, spacing: 2) {
            Text(target.name)
            if !target.does.isEmpty {
                Text(target.does)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        #endif
    }
}

/// What a profile is for, and for a correction, the target it corrects to:
/// the target belongs to the correction, whose job is to make the headphone
/// neutral, and anything more is said against neutral.
private struct RoleSection: View {
    let dsp: DspModel
    let detail: DspProfileDetail
    /// Every target, for what a ready-made EQ was made for.
    let madeForChoices: [DspTargetOption]
    /// The targets this correction can move to, once its own is known.
    let targets: DspTargets?
    @Binding var adding: Bool

    private var madeFor: String? { targets?.madeFor?.id }
    private var current: String { targets?.chosen ?? madeFor ?? "" }

    var body: some View {
        Section {
            Picker("This profile is", selection: Binding(
                get: { detail.role },
                set: { dsp.setRole(detail.name, $0) }
            )) {
                Text("A neutral correction for these headphones").tag(DspRole.correction)
                Text("A correction with a sound already in it").tag(DspRole.baked)
                Text("A tuning to add on top").tag(DspRole.tuning)
            }
            if detail.role == .correction {
                if let targets {
                    Picker("Corrected to", selection: Binding(
                        get: { current },
                        set: { id in dsp.chooseTarget(detail.name, id == madeFor ? nil : id) }
                    )) {
                        ForEach(targets.choices, id: \.id) { c in
                            TargetRow(target: c).tag(c.id)
                        }
                    }
                    #if os(iOS)
                    .pickerStyle(.navigationLink)
                    #endif
                    if let c = targets.choices.first(where: { $0.id == current }), !c.character.isEmpty {
                        Text(c.character)
                            .font(.callout)
                            .foregroundStyle(.secondary)
                    }
                    #if !os(tvOS)
                    Button("Add a Target…") { adding = true }
                    #endif
                }
                if targets == nil, !detail.measured, !madeForChoices.isEmpty {
                    Picker("Made for", selection: Binding(
                        get: { detail.madeFor ?? "" },
                        set: { dsp.setMadeFor(detail.name, $0.isEmpty ? nil : $0) }
                    )) {
                        Text("Unknown").tag("")
                        ForEach(madeForChoices, id: \.id) { t in
                            TargetRow(target: t).tag(t.id)
                        }
                    }
                    #if os(iOS)
                    .pickerStyle(.navigationLink)
                    #endif
                }
            }
        } header: {
            Text("What it's for")
        } footer: {
            Text(footer)
                .font(.caption)
                .foregroundStyle(.tertiary)
        }
    }

    private var footer: String {
        switch detail.role {
        case .tuning:
            return "A tuning is taste: more bass, a darker treble. It plays on top of a correction."
        case .baked:
            return "A correction with a tuning already in it, as most finished presets are. It counts as the stack's correction, so a tuning on top would add taste twice."
        case .correction:
            break
        }
        if detail.measured {
            return "A correction makes your headphones neutral, and the target says what neutral is. This one is worked out again from the measurement for each target."
        }
        if let made = targets?.madeFor {
            return "Made for \(made.name). Another target is worked out from the measurement AutoEQ kept, where there is one, or plays as the difference between the two. Moving from Harman to neutral takes Harman's bass and treble out."
        }
        return "A correction makes your headphones neutral. Say which target this EQ was made for, and you can move it to another; if you don't know, leave it Unknown and target switching stays off."
    }
}

private struct ImpulseRow: View {
    let ir: DspImpulse

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack {
                Text("\(DspModel.khz(ir.rate)) kHz")
                Spacer()
                Text(channels)
                    .foregroundStyle(.secondary)
            }
            Text(shape)
                .font(.caption)
                .foregroundStyle(.secondary)
            Text(ir.file)
                .font(.caption)
                .foregroundStyle(.tertiary)
        }
    }

    private var channels: String {
        switch ir.channels {
        case nil: "Every channel"
        case 1: "Mono"
        case 2: "Stereo"
        case let n?: "\(n) channels"
        }
    }

    private var shape: String {
        let seconds = Double(ir.taps) / Double(ir.rate)
        var parts = [
            "\(ir.taps.formatted()) taps (\(String(format: "%.2f", seconds)) s)",
            "peaks at \(String(format: "%.1f", ir.peakMs)) ms",
        ]
        if ir.mixes { parts.append("mixes channels") }
        if ir.delayed { parts.append("delays channels") }
        return parts.joined(separator: " · ")
    }
}

/// Where a profile is kept: on every device signed in to the account's kōan
/// server, or on this one alone.
private struct ScopeSection: View {
    let dsp: DspModel
    let detail: DspProfileDetail

    var body: some View {
        Section {
            Picker("Sync", selection: Binding(
                get: { detail.everywhere },
                set: { dsp.setScope(detail.name, everywhere: $0) }
            )) {
                Text("Everywhere").tag(true)
                Text("This device").tag(false)
            }
            if let problem = detail.syncProblem {
                Label(problem, systemImage: "exclamationmark.icloud")
                    .foregroundStyle(.orange)
            }
            if let note = detail.syncNote {
                Label(note, systemImage: "arrow.triangle.2.circlepath")
                    .font(.callout)
                    .foregroundStyle(.secondary)
            }
        } header: {
            Text("Sync")
        } footer: {
            Text(detail.everywhere
                 ? "Everywhere: kept on every device signed in to your kōan server, and an edit on one reaches the rest. Which output plays it stays each device's own. Headphone corrections sync by default, since headphones move between devices."
                 : "This device: never leaves it. Room and speaker corrections stay by default, since they belong to where they were measured. Moving a profile here from everywhere removes it from your other devices.")
                .font(.caption)
                .foregroundStyle(.tertiary)
        }
    }
}

struct BandRow: View {
    let band: DspBand

    var body: some View {
        HStack {
            Text(kind)
            Spacer()
            Text(values)
                .monospacedDigit()
                .foregroundStyle(.secondary)
        }
    }

    private var kind: String {
        let name = switch band.kind {
        case "peaking": "Peaking"
        case "low_shelf": "Low shelf"
        case "high_shelf": "High shelf"
        case "low_pass": "Low pass"
        case "high_pass": "High pass"
        case "notch": "Notch"
        case "band_pass": "Band pass"
        case "all_pass": "All pass"
        case "low_shelf_first_order": "Low shelf, 6 dB/oct"
        case "high_shelf_first_order": "High shelf, 6 dB/oct"
        case "low_pass_first_order": "Low pass, 6 dB/oct"
        case "high_pass_first_order": "High pass, 6 dB/oct"
        case "all_pass_first_order": "All pass, first order"
        case "gain": "Gain"
        case "delay": "Delay"
        case "mix": "Mix"
        case "graphic": "Graphic EQ"
        default: band.kind
        }
        let channels = band.channels.map(Self.channel)
        return channels.isEmpty ? name : "\(name) · \(channels.joined(separator: " "))"
    }

    private static func channel(_ c: UInt16) -> String {
        switch c {
        case 0: "L"
        case 1: "R"
        default: "Ch \(c + 1)"
        }
    }

    private var values: String {
        let gain = String(format: "%+.1f dB", band.gainDb)
        switch band.kind {
        case "gain":
            return gain
        case "delay":
            var parts: [String] = []
            if band.delayMs != 0 { parts.append(String(format: "%.2f ms", band.delayMs)) }
            if band.delaySamples != 0 { parts.append(String(format: "%.2f samples", band.delaySamples)) }
            return parts.joined(separator: " + ")
        case "mix":
            // Only the outputs the mix changes.
            return band.mix.enumerated().compactMap { o, out in
                let s = out.sources
                if s.count == 1, s[0].channel == UInt16(o), s[0].gain == 1 { return nil }
                let terms = s.map { src in
                    src.gain == 1 ? Self.channel(src.channel) : String(format: "%g×%@", src.gain, Self.channel(src.channel))
                }
                return "\(Self.channel(UInt16(o))) = \(terms.isEmpty ? "0" : terms.joined(separator: " + "))"
            }.joined(separator: "   ")
        case "graphic":
            let gains = band.curve.map(\.db)
            return String(format: "%d points  %+.1f to %+.1f dB", band.curve.count, gains.min() ?? 0, gains.max() ?? 0)
        default:
            break
        }
        let freq = band.freq >= 1000
            ? String(format: "%.1f kHz", band.freq / 1000)
            : String(format: "%.0f Hz", band.freq)
        let q = String(format: "Q %.2f", band.q)
        if band.kind.hasSuffix("_first_order") {
            return band.kind.contains("shelf") ? "\(freq)  \(gain)" : freq
        }
        return ["low_pass", "high_pass", "notch", "band_pass", "all_pass"].contains(band.kind)
            ? "\(freq)  \(q)"
            : "\(freq)  \(gain)  \(q)"
    }
}
