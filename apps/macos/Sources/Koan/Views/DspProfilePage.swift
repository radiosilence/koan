import KoanFFI
import SwiftUI

/// One profile, and exactly what is in it: each impulse response's rate,
/// channels, length and routing, any bands, the headroom it is given, and the
/// outputs that play through it. Renamed and deleted from here.
struct DspProfilePage: View {
    let dsp: DspModel
    @State var name: String
    /// The output whose tuning a copy saved here takes this EQ's place in;
    /// the one in use if none.
    var device: String?
    @Environment(\.dismiss) private var dismiss

    @State private var detail: DspProfileDetail?
    @State private var response: DspResponse?
    @State private var targets: DspTargets?
    /// Every target, for what a ready-made EQ was made for.
    @State private var madeForChoices = TargetGroups()
    /// For a tuning: the target it looks made against, while it does not
    /// say, and what it adds on the output's correction for each choice.
    @State private var suggestion: DspTargetName?
    @State private var previews: [String: [Double]] = [:]
    @State private var addingTarget = false
    @State private var editingName = ""
    @State private var confirmingDelete = false
    /// Bands, responses, headroom and sync, for a correction: most people
    /// pick a correction and its target and are done.
    @State private var showingMore = false
    @State private var splitting = false
    /// The EQ as it was when the page opened, which Save as Copy puts back,
    /// and as it is now.
    @State private var before: String?
    @State private var now: String?
    @State private var copying = false
    @State private var copyName = ""
    @State private var confirmingReset = false

    var body: some View {
        KoanForm {
            if let d = detail {
                Section {
                    ChainSummaryCard(corrects: d.corrects, baked: d.correctsBaked, tunings: d.tunings)
                    if let twice = d.correctsTwice {
                        Label(twice, systemImage: "exclamationmark.triangle.fill")
                            .foregroundStyle(KoanTheme.style(.bad, system: .orange))
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
                        EqEditor(dsp: dsp, name: name, detail: d, response: r, parts: onCorrection(d, r))
                    }
                }
                #if !os(tvOS)
                if !d.readOnly, before != now || (d.canRevert && d.edited) {
                    keeping(d)
                }
                #endif
                if let problem = d.problem {
                    Section {
                        Label(problem, systemImage: "exclamationmark.triangle.fill")
                            .foregroundStyle(KoanTheme.style(.bad, system: .orange))
                    }
                }

                Section {
                    if d.devices.isEmpty {
                        Text("No output yet")
                            .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
                            .koanCase()
                    }
                    ForEach(d.devices, id: \.self) { Text(dsp.label($0)) }
                    if let device = dsp.overview?.device {
                        // The app's words follow the theme; the device's name
                        // keeps its case.
                        if d.devices.contains(device) {
                            Button { dsp.use(nil) } label: {
                                Text("\(KoanTheme.label("Stop using for")) \(dsp.label(device))").textCase(nil)
                            }
                        } else {
                            Button { dsp.use(d.name) } label: {
                                Text("\(KoanTheme.label("Use for")) \(dsp.label(device))").textCase(nil)
                            }
                        }
                    }
                } header: {
                    KoanSectionHeader("Used for")
                }

                // A stack with nothing of its own is what its layers are.
                if d.layers.isEmpty || !d.bands.isEmpty || !d.impulses.isEmpty {
                    RoleSection(dsp: dsp, detail: d, madeForChoices: madeForChoices,
                                targets: targets, suggestion: suggestion, previews: previews,
                                adding: $addingTarget, splitting: $splitting)
                }
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
                                    .foregroundStyle(KoanTheme.style(.muted, system: .tertiary))
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
                    Button("Delete", role: .destructive) { confirmingDelete = true }
                }
            } else {
                ProgressView()
            }
        }
        .navigationTitle(name)
        .task(id: "\(name)\u{0}\(dsp.stamp)") { await load() }
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
        .sheet(isPresented: $splitting) {
            SplitFlow(dsp: dsp, name: name).koanSheet()
        }
        #endif
        #if !os(tvOS)
        .alert("Save as Copy", isPresented: $copying) {
            TextField("Name", text: $copyName)
            Button("Save") { saveCopy() }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("The copy takes this EQ's place in the tuning, and this one goes back to how it was.")
        }
        .confirmationDialog("Reset \(name) to its file?", isPresented: $confirmingReset, titleVisibility: .visible) {
            Button("Reset", role: .destructive) {
                dsp.revert(name)
                before = nil
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

    /// Keeping an edit apart from what was there: as a copy, with this EQ put
    /// back, or for an import, going back to its file.
    private func keeping(_ d: DspProfileDetail) -> some View {
        Section {
            Button("Save as Copy…") {
                copyName = "\(name) copy"
                copying = true
            }
            .koanButton(.bordered)
            if d.canRevert, d.edited {
                Button("Reset to File") { confirmingReset = true }
                    .koanButton(.bordered)
            }
        } footer: {
            Text(d.canRevert
                 ? "Save as Copy keeps this edit as an EQ of its own, in this one's place in the tuning, and puts this one back. Reset to File puts it back as \(d.source.first ?? "its file") had it."
                 : "Save as Copy keeps this edit as an EQ of its own, in this one's place in the tuning, and puts this one back as it was when you opened it.")
                .koanText(.fine, .muted)
        }
    }

    private func saveCopy() {
        let (from, kept, on) = (name, before, device ?? dsp.overview?.device)
        let new = copyName.trimmingCharacters(in: .whitespaces)
        Task {
            if let copy = await dsp.saveAsCopy(from, as: new.isEmpty ? nil : new, before: kept, device: on) {
                before = nil
                name = copy
            }
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
                KoanSectionHeader("Impulse responses")
            } footer: {
                Text("A track at a rate with no response of its own is resampled to the nearest one here.")
                    .font(.role(.fine, system: .caption))
                    .foregroundStyle(KoanTheme.style(.muted, system: .tertiary))
            }
        }

        BandTable(dsp: dsp, profile: name, bands: d.bands, readOnly: d.readOnly)

        Section {
            LabeledContent("Preamp", value: "\(String(format: "%.1f", d.preampDb)) dB")
        } header: {
            KoanSectionHeader("Headroom")
        } footer: {
            Text(d.preampSet
                 ? "Set in this EQ."
                 : "Derived at \(DspModel.khz(d.preampRate)) kHz from the largest gain the filters apply, so nothing they boost can clip.")
                .font(.role(.fine, system: .caption))
                .foregroundStyle(KoanTheme.style(.muted, system: .tertiary))
        }

        if !d.source.isEmpty {
            Section {
                ForEach(d.source, id: \.self) { Text($0).foregroundStyle(KoanTheme.style(.muted, system: .secondary)) }
            } header: {
                KoanSectionHeader("Imported from")
            }
        }
    }

    private func load() async {
        let asked = name
        let snapshot = await dsp.snapshot(asked)
        guard asked == name else { return }
        now = snapshot
        if before == nil { before = snapshot }
        detail = await dsp.detail(name)
        response = await dsp.response(name)
        targets = await dsp.targets(name)
        madeForChoices = await TargetGroups(
            over: dsp.targetsFor(inEar: false),
            inEar: dsp.targetsFor(inEar: true),
            first: dsp.overview?.inEar
        )
        if detail?.role == .tuning {
            suggestion = await dsp.suggestMadeAgainst(name)
            previews = await dsp.madeAgainstPreviews(name)
        } else {
            suggestion = nil
            previews = [:]
        }
        editingName = name
    }

    /// A tuning as it plays on the output's correction, where that differs
    /// from the tuning alone: with the target difference its Made against
    /// asks for, so a wrong choice shows as a preference doubled or taken out.
    private func onCorrection(_ d: DspProfileDetail, _ r: DspResponse) -> [EqGraph.Part] {
        guard let db = previews[d.tunedFor ?? ""], db.count == r.total.count, db != r.total else { return [] }
        return [EqGraph.Part(name: "On the correction in use", db: db, stroke: .eq(1))]
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
            Button("Play Them All in Order") { dsp.setGroup(detail.name, false) }
            #endif
        } header: {
            KoanSectionHeader("Group: pick one")
        } footer: {
            Text(detail.layers.contains(where: \.on)
                 ? "One member plays at a time. Pick another and it plays in place of the last. Each member is an EQ of its own, with its own page."
                 : "None was picked, so the first plays. Pick one to change it.")
                .font(.role(.fine, system: .caption))
                .foregroundStyle(KoanTheme.style(.muted, system: .tertiary))
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
        Menu(layers.isEmpty ? "Add a Tuning…" : "Add an EQ") {
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
                    Button("Remove", role: .destructive) { remove(index) }
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
            KoanSectionHeader("Plays first")
        } footer: {
            Text("Played in order, before this EQ's own bands. One switched off plays nothing.")
                .font(.role(.fine, system: .caption))
                .foregroundStyle(KoanTheme.style(.muted, system: .tertiary))
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
        case .baked: "Correction + Tuning"
        }
    }

    /// The role's colour: the accent for a correction, and in the theme ink
    /// and muted for the others, so the accent stays the curve that corrects.
    var color: AnyShapeStyle {
        switch self {
        case .correction: AnyShapeStyle(.tint)
        case .tuning: KoanTheme.style(.ink, system: Color.orange)
        case .baked: KoanTheme.style(.muted, system: Color.purple)
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
            .font(.role(.fine, system: .caption2.weight(.semibold)))
            .foregroundStyle(role.color)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(role.color.opacity(0.15), in: .rect(cornerRadius: KoanTheme.radius(8)))
    }
}

/// Every target, by the headphones it is for, and those added once: each
/// list of targets for an ear ends with the added ones, and a picker with a
/// row twice cannot choose either. Neutral is a target of both ears, so a
/// target's ear is said by the heading it is under.
struct TargetGroups {
    var over: [DspTargetOption] = []
    var inEar: [DspTargetOption] = []
    var added: [DspTargetOption] = []

    init() {}

    /// Whether the device's correction is for in-ears, which puts its kind
    /// first and the other under Other; none shows both as they are.
    var first: Bool?

    init(over: [DspTargetOption], inEar: [DspTargetOption], first: Bool? = nil) {
        let both = Set(over.map(\.id)).intersection(inEar.map(\.id))
        self.over = over.filter { !both.contains($0.id) }
        self.inEar = inEar.filter { !both.contains($0.id) }
        added = over.filter { both.contains($0.id) }
        self.first = first
    }

    var isEmpty: Bool { over.isEmpty && inEar.isEmpty && added.isEmpty }

    /// The targets' ids under a heading for each group, Unknown first.
    var sections: [(title: String?, values: [String])] {
        let groups: [(String, [DspTargetOption])] = switch first {
        case true?: [("In-ear", inEar), ("Other", over), ("Added", added)]
        case false?: [("Over-ear", over), ("Other", inEar), ("Added", added)]
        case nil: [("Over-ear", over), ("In-ear", inEar), ("Added", added)]
        }
        return [(nil, [""])] + groups
            .filter { !$0.1.isEmpty }
            .map { ($0.0, $0.1.map(\.id)) }
    }

    func option(_ id: String) -> DspTargetOption? {
        (over + inEar + added).first { $0.id == id }
    }

    /// A target's row, or Unknown for none.
    @ViewBuilder func row(_ id: String) -> some View {
        if let t = option(id) {
            TargetRow(target: t)
        } else {
            Text("Unknown")
        }
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
    /// The target the correction was made for, which it plays unless moved.
    var isDefault = false

    private var name: String { isDefault ? "\(target.name) (default)" : target.name }

    var body: some View {
        #if os(macOS)
        Text(target.does.isEmpty ? name : "\(name): \(target.does)")
        #else
        VStack(alignment: .leading, spacing: 2) {
            Text(name)
            if !target.does.isEmpty {
                Text(target.does)
                    .font(.role(.fine, system: .caption))
                    .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
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
    let madeForChoices: TargetGroups
    /// The targets this correction can move to, once its own is known.
    let targets: DspTargets?
    let suggestion: DspTargetName?
    /// What a tuning adds on the output's correction, by Made against.
    let previews: [String: [Double]]
    @Binding var adding: Bool
    /// Taking a baked EQ apart, presented by the page.
    @Binding var splitting: Bool
    /// The targets just picked, by picker, until the reloaded detail says
    /// them. Saving is asynchronous, and a phone's picker list whose
    /// selection reads back unchanged stays open with the old row ticked.
    @State private var picked: [String: String] = [:]

    private var madeFor: String? { targets?.madeFor?.id }
    private var current: String { targets?.chosen ?? madeFor ?? "" }

    private func choice(_ picker: String, saved: String, save: @escaping (String) -> Void) -> Binding<String> {
        Binding(
            get: { picked[picker] ?? saved },
            set: {
                picked[picker] = $0
                save($0)
            }
        )
    }

    /// One scale for every choice's curve, so a doubled shelf stands taller.
    private var previewRange: Double {
        max(6, previews.values.flatMap { $0 }.map(abs).max() ?? 0)
    }

    /// A Made against choice, and on a phone, what the tuning adds on the
    /// correction in use with it.
    private func madeAgainstRow(_ id: String) -> some View {
        HStack(spacing: KoanTheme.Space.m) {
            madeForChoices.row(id)
            #if os(iOS)
            Spacer(minLength: 0)
            if let db = previews[id] {
                CurveThumb(db: db, stroke: .eq(1), range: previewRange)
            }
            #endif
        }
    }

    var body: some View {
        Section {
            KoanPicker("This is", selection: Binding(
                get: { detail.role },
                set: { dsp.setRole(detail.name, $0) }
            ), options: [DspRole.correction, .baked, .tuning].map { (ProfileRole($0).label, $0) })
            if detail.role == .tuning, !madeForChoices.isEmpty {
                KoanListPicker(
                    title: "Made against",
                    selection: choice("made against", saved: detail.tunedFor ?? "") {
                        dsp.setTunedFor(detail.name, $0.isEmpty ? nil : $0)
                    },
                    sections: madeForChoices.sections,
                    name: { madeForChoices.option($0)?.name ?? "Unknown" },
                    row: madeAgainstRow
                )
                if detail.tunedFor == nil, let suggestion {
                    Button("Looks made for \(suggestion.name). Use that?") {
                        dsp.setTunedFor(detail.name, suggestion.id)
                    }
                    .koanButton(.link)
                }
            }
            #if !os(tvOS)
            if detail.role == .baked, detail.impulses.isEmpty, detail.layers.isEmpty {
                Button("Split into Correction + Tuning…") { splitting = true }
            }
            #endif
            if detail.role == .correction {
                if let targets {
                    KoanListPicker(
                        title: "Corrected to",
                        selection: choice("corrected to", saved: current) { id in
                            dsp.chooseTarget(detail.name, id == madeFor ? nil : id)
                        },
                        sections: [(nil, targets.choices.map(\.id))],
                        name: { id in targets.choices.first { $0.id == id }?.name ?? "" }
                    ) { id in
                        if let c = targets.choices.first(where: { $0.id == id }) {
                            TargetRow(target: c)
                        }
                    }
                    if let c = targets.choices.first(where: { $0.id == (picked["corrected to"] ?? current) }), !c.character.isEmpty {
                        Text(c.character)
                            .font(.role(.control, system: .callout))
                            .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
                    }
                    #if !os(tvOS)
                    Button("Add a Target…") { adding = true }
                    #endif
                }
                if targets == nil, !detail.measured, !madeForChoices.isEmpty {
                    KoanListPicker(
                        title: "Made for",
                        selection: choice("made for", saved: detail.madeFor ?? "") {
                            dsp.setMadeFor(detail.name, $0.isEmpty ? nil : $0)
                        },
                        sections: madeForChoices.sections,
                        name: { madeForChoices.option($0)?.name ?? "Unknown" },
                        row: madeForChoices.row
                    )
                }
            }
        } header: {
            KoanSectionHeader("What it's for")
        } footer: {
            Text(footer)
                .font(.role(.fine, system: .caption))
                .foregroundStyle(KoanTheme.style(.muted, system: .tertiary))
        }
        .onChange(of: [detail.name, detail.tunedFor, detail.madeFor, targets?.chosen]) { picked = [:] }
    }

    private var footer: String {
        switch detail.role {
        case .tuning:
            return "A tuning is taste: more bass, a darker treble. It plays on top of a correction. Say which target it was made against, and on a device corrected to another, kōan plays the difference first, so it sounds as it was made to."
        case .baked:
            return "A correction with a tuning already in it, as most finished presets are. It is the device's correction, so another tuning on top would add taste twice. Split it, with a measurement of the device, to change the tuning."
        case .correction:
            break
        }
        if detail.measured {
            return "A correction makes your headphones or speakers neutral, and the target says what neutral is. This one is worked out again from the measurement for each target."
        }
        if let made = targets?.madeFor {
            return "Made for \(made.name). Another target is worked out from the measurement AutoEQ kept, where there is one, or plays as the difference between the two. Moving from Harman to neutral takes Harman's bass and treble out."
        }
        return "A correction makes your headphones or speakers neutral. Say which target this EQ was made for, and you can move it to another; if you don't know, leave it Unknown and target switching stays off."
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
                    .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
            }
            Text(shape)
                .font(.role(.fine, system: .caption))
                .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
            Text(ir.file)
                .font(.role(.fine, system: .caption))
                .foregroundStyle(KoanTheme.style(.muted, system: .tertiary))
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
            KoanPicker("Sync", selection: Binding(
                get: { detail.everywhere },
                set: { dsp.setScope(detail.name, everywhere: $0) }
            ), options: [("Everywhere", true), ("This device", false)])
            if let problem = detail.syncProblem {
                Label(problem, systemImage: "exclamationmark.icloud")
                    .foregroundStyle(KoanTheme.style(.bad, system: .orange))
            }
            if let note = detail.syncNote {
                Label(note, systemImage: "arrow.triangle.2.circlepath")
                    .font(.role(.control, system: .callout))
                    .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
            }
        } header: {
            KoanSectionHeader("Sync")
        } footer: {
            Text(detail.everywhere
                 ? "Everywhere: kept on every device signed in to your kōan server, and an edit on one reaches the rest. Which output plays it stays each device's own. Headphone corrections sync by default, since headphones move between devices."
                 : "This device: never leaves it. Room and speaker corrections stay by default, since they belong to where they were measured. Moving one here from everywhere removes it from your other devices.")
                .font(.role(.fine, system: .caption))
                .foregroundStyle(KoanTheme.style(.muted, system: .tertiary))
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
                .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
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
