import KoanFFI
import SwiftUI

/// EQ for a device, as the chain it plays: music in, its correction, its
/// tuning's EQs in order, the device out. The device and its preset head the
/// page, then the curve of the whole chain, always the same height, then the
/// chain itself, each stage opening where it is chosen or edited.
struct EqSettings: View {
    @Environment(AppState.self) private var app
    /// The device shown: the output in use until another is picked.
    @State private var picked: String?
    /// Manage EQ is open over the page.
    @State private var managing = false

    /// Open on `device`, or on the output in use.
    init(device: String? = nil) {
        _picked = State(initialValue: device)
    }
    @State private var overview: DspOverview?
    @State private var response: DspResponse?
    /// The curve each stage draws alone, by profile name.
    @State private var curves: [String: [Double]] = [:]
    // What the page presents is held here and presented from the form: a
    // modifier on a section of a list is applied to each of its rows, and
    // the presentation ends when that row is made again.
    @State private var importing = false
    /// The stage whose Import… the file picker was opened for.
    @State private var importStage: Stage?
    /// What a stage's picker asked for, presented once the picker has gone:
    /// a sheet asked for while another is leaving is never shown.
    @State private var adding: (Stage, StageAdd)?
    @State private var finding: AutoEqFind?
    @State private var measuring = false
    @State private var splitting: ShownProfile?
    @State private var showing: ShownProfile?
    @State private var choosing: Stage?
    @State private var explaining = false
    @State private var naming = false
    @State private var namingTitle = "Save as Preset"
    @State private var presetName = ""
    /// Why the name given for a new preset was not taken.
    @State private var refusal: String?

    private var device: String? { picked ?? overview?.device }

    /// Whether the device plays untouched: nothing chosen at all.
    private var flat: Bool {
        guard let o = overview else { return true }
        return o.active == nil && o.chain.isEmpty
    }

    var body: some View {
        KoanForm {
            if let o = overview, let device {
                head(o, device)
                Section {
                    graph
                        .frame(height: 290, alignment: .top)
                }
                Section {
                    EqChain(
                        overview: o,
                        device: app.dsp.label(device),
                        aim: aim,
                        curves: curves,
                        choose: { choosing = $0 },
                        open: { showing = ShownProfile(name: $0) },
                        set: { app.dsp.setTunings($0, for: device) }
                    )
                    if let leftOut = o.leftOut {
                        Label(leftOut, systemImage: "exclamationmark.triangle")
                            .koanText(.meta, .bad)
                    }
                } footer: {
                    Text(EqChain.sentence(o, device: app.dsp.label(device), aim: aim))
                        .koanText(.fine, .muted)
                }
            }
            Section {
                #if !os(tvOS)
                if let offer = app.dsp.suggestion, device == app.dsp.overview?.device {
                    AutoEqSuggestion(offer: offer, dsp: app.dsp) { query in
                        finding = AutoEqFind(query: query)
                    }
                }
                #endif
                if let summary = app.dsp.importSummary {
                    Text(summary).koanText(.fine, .muted)
                }
                if let error = app.dsp.lastError {
                    Text(error).koanText(.fine, .bad)
                }
                Button("Manage EQ") { managing = true }
                    .koanButton(.text)
            }
        }
        .koanSheet()
        #if os(macOS)
        // A sheet, as each EQ's page is: a pane of the Settings window has no
        // stack to go into, and one pushed there has no way back.
        .sheet(isPresented: $managing) {
            NavigationStack {
                ManageEq(device: device, active: overview?.active, chain: overview?.chain ?? [])
                    .toolbar {
                        ToolbarItem(placement: .confirmationAction) {
                            Button("Done") { managing = false }
                        }
                    }
            }
            .frame(minWidth: 480, minHeight: 520)
        }
        #else
        .navigationDestination(isPresented: $managing) {
            ManageEq(device: device, active: overview?.active, chain: overview?.chain ?? [])
                .koanBackButton()
                .koanHidesSystemTabBar()
        }
        #endif
        #if os(macOS)
        .onChange(of: app.dsp.editing, initial: true) { _, asked in
            guard let asked else { return }
            picked = asked == app.dsp.overview?.device ? nil : asked
            app.dsp.editing = nil
        }
        #endif
        .task(id: "\(picked ?? "")\u{0}\(app.dsp.stamp)") { await load() }
        .task(id: app.dsp.stamp) { app.dsp.reload() }
        #if !os(tvOS)
        .filePicker(
            isPresented: $importing,
            allowedContentTypes: [.item, .folder],
            allowsMultipleSelection: true
        ) { result in
            let stage = importStage
            importStage = nil
            if case let .success(urls) = result, !urls.isEmpty, let stage, let device {
                app.dsp.importFiles(urls, into: DspPlacement(device: device, stage: stage))
            }
        }
        .sheet(item: $finding) { find in
            AutoEqSearch(dsp: app.dsp, query: find.query).koanSheet()
        }
        .sheet(isPresented: $measuring) {
            MeasurementFlow(dsp: app.dsp).koanSheet()
        }
        .sheet(item: $splitting) { baked in
            SplitFlow(dsp: app.dsp, name: baked.name).koanSheet()
        }
        .sheet(item: $choosing, onDismiss: {
            guard let (stage, add) = adding else { return }
            adding = nil
            switch add {
            case .importing:
                importStage = stage
                importing = true
            case .autoEq: finding = AutoEqFind(query: "")
            case .measuring: measuring = true
            case let .splitting(name): splitting = ShownProfile(name: name)
            }
        }) { stage in
            if let o = overview, let device {
                StagePicker(dsp: app.dsp, stage: stage, overview: o, device: device) { add in
                    adding = (stage, add)
                    choosing = nil
                }
                .koanSheet()
            }
        }
        .sheet(isPresented: $explaining) {
            EqExplainer().koanSheet()
        }
        // A profile imported from a file: a neutral correction, one with a
        // tuning already in it, or taste to add on top? kōan cannot tell,
        // and a chain corrects once.
        .sheet(item: Binding(
            // Manage EQ asks for its own imports while it is open.
            get: { managing ? nil : app.dsp.askRole },
            // Swiped away, as Decide Later.
            set: { if $0 == nil, let ask = app.dsp.askRole { app.dsp.answer(ask, nil) } }
        )) { ask in
            RoleQuestion(dsp: app.dsp, ask: ask).koanSheet()
        }
        .alert(namingTitle, isPresented: $naming) {
            TextField("Name", text: $presetName)
            Button("Save") { save(as: presetName, over: false) }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("The correction and tuning, to switch \(device.map(app.dsp.label) ?? "a device") back to, or another device to.")
        }
        #endif
        #if os(iOS)
        .navigationDestination(item: $showing) { shown in
            DspProfilePage(dsp: app.dsp, name: shown.name)
                .koanBackButton()
                .koanHidesSystemTabBar()
        }
        #elseif os(macOS)
        .sheet(item: $showing) { shown in
            NavigationStack {
                DspProfilePage(dsp: app.dsp, name: shown.name)
                    .toolbar {
                        ToolbarItem(placement: .confirmationAction) {
                            Button("Done") { showing = nil }
                        }
                    }
            }
            .frame(minWidth: 480, minHeight: 440)
        }
        #endif
    }

    // MARK: - Head

    /// The device, and the preset it was set from, or Flat, or Unsaved.
    @ViewBuilder private func head(_ o: DspOverview, _ device: String) -> some View {
        Section {
            KoanPicker(
                "Device",
                selection: Binding(
                    get: { device },
                    set: { picked = $0 == app.dsp.overview?.device ? nil : $0 }
                ),
                options: devices(o).map { d in
                    (d == app.dsp.overview?.device ? "\(app.dsp.label(d)) (\(KoanTheme.label("In use")))" : app.dsp.label(d), d)
                },
                keepsCase: true
            )
            KoanPicker(
                "Preset",
                selection: Binding(
                    get: {
                        guard let preset = o.preset else { return flat ? Self.flatTag : Self.unsavedTag }
                        return o.presetEdited ? Self.editedTag : preset
                    },
                    set: { tag in
                        guard tag != Self.unsavedTag, tag != Self.editedTag else { return }
                        app.dsp.applyPreset(tag == Self.flatTag ? nil : tag, to: device)
                    }
                ),
                options: [(KoanTheme.label("Flat"), Self.flatTag)]
                    + (o.preset == nil && !flat ? [(KoanTheme.label("Unsaved"), Self.unsavedTag)] : [])
                    // A preset changed since: as edited, chosen, and as saved,
                    // which goes back to it.
                    + presets(o).flatMap { name in
                        (name == o.preset && o.presetEdited
                            ? [("\(name) (\(KoanTheme.label("edited")))", Self.editedTag)]
                            : []) + [(name, name)]
                    },
                keepsCase: true
            )
            #if !os(tvOS)
            if let preset = o.preset, o.presetEdited {
                LabeledContent {
                    HStack {
                        Button("Save") { save(as: preset, over: true) }
                            .koanButton(.compact)
                        Button("Revert") { app.dsp.applyPreset(preset, to: device) }
                            .koanButton(.text)
                        Button("Save as New…") { ask("Save as New Preset") }
                            .koanButton(.text)
                    }
                } label: {
                    // The preset's name keeps its case; the words are the app's.
                    Text("\(KoanTheme.label("Changed since")) \(preset)").textCase(nil)
                }
            } else if o.preset == nil, !flat {
                Button("Save as Preset…") { ask("Save as Preset") }
                    .koanButton(.compact)
            }
            if let refusal {
                Text(refusal).koanText(.fine, .bad)
            }
            #endif
        } header: {
            HStack {
                KoanSectionHeader("Device and preset")
                Spacer()
                #if !os(tvOS)
                Button { explaining = true } label: {
                    Label("How EQ works", systemImage: "info.circle")
                }
                .koanButton(.text)
                #endif
            }
        }
    }

    private static let flatTag = "\u{0}flat"
    private static let unsavedTag = "\u{0}unsaved"
    private static let editedTag = "\u{0}edited"

    /// The outputs to choose among: the one in use first, then this Mac's,
    /// then any the EQ names.
    private func devices(_ o: DspOverview) -> [String] {
        var all: [String] = []
        func add(_ d: String?) {
            if let d, !all.contains(d) { all.append(d) }
        }
        add(app.dsp.overview?.device)
        add(device)
        #if os(macOS)
        app.player.devices.forEach { add($0.name) }
        #endif
        o.profiles.filter { !$0.preset }.flatMap(\.devices).forEach(add)
        o.tunings.keys.sorted().forEach(add)
        return all
    }

    private func presets(_ o: DspOverview) -> [String] {
        o.profiles.filter(\.preset).map(\.name)
    }

    private func ask(_ title: String) {
        namingTitle = title
        presetName = ""
        refusal = nil
        naming = true
    }

    /// Save the chain as the preset `name`. A new name never writes over
    /// another preset: only Save, `over`, changes one.
    private func save(as name: String, over: Bool) {
        let name = name.trimmingCharacters(in: .whitespaces)
        guard let device, let o = overview, !name.isEmpty else { return }
        if !over, presets(o).contains(name) {
            refusal = "There is already a preset called \(name). Choose another name, or change it with Save."
            return
        }
        refusal = nil
        Task { _ = await app.dsp.savePreset(name, from: device) }
    }

    /// A flat chain, drawn: no change anywhere.
    private static let flatResponse: DspResponse = {
        let freqs = (0 ..< 120).map { 20 * pow(1000, Double($0) / 119) }
        return DspResponse(
            freqs: freqs, total: freqs.map { _ in 0 }, bands: [], layers: [],
            measurement: nil, target: nil, predicted: nil, preampDb: 0,
            correction: nil, tuning: nil, original: nil
        )
    }()

    // MARK: - The curve

    /// Always drawn: a flat device is a line at 0 dB, and says so.
    @ViewBuilder private var graph: some View {
        if let response, !flat {
            EqGraph(response: response, parts: parts(response))
                .koanAnimation(KoanTheme.Motion.normal, value: response.total)
        } else {
            EqGraph(response: Self.flatResponse)
                .overlay(alignment: .top) {
                    Text("Flat: plays untouched")
                        .koanText(.meta, .muted)
                        .koanCase()
                        .padding(.top, KoanTheme.Space.xl)
                }
        }
    }

    /// Each stage playing, drawn as its block is.
    private func parts(_ r: DspResponse) -> [EqGraph.Part] {
        guard let o = overview else { return [] }
        var parts: [EqGraph.Part] = []
        let eqs = o.chain.enumerated().filter(\.element.on).compactMap { i, entry in
            r.layers.first { $0.name == entry.name }.map { (i, entry.name, $0.db) }
        }
        if let active = o.active, let db = r.correction ?? (eqs.isEmpty ? r.total : nil) {
            parts.append(EqGraph.Part(name: active, db: db, stroke: .correction))
        }
        for (i, name, db) in eqs {
            parts.append(EqGraph.Part(name: name, db: db, stroke: .eq(i)))
        }
        return parts
    }

    /// The target the correction aims at, for the sentence.
    @State private var aim: String?

    private func load() async {
        let o = await app.dsp.overview(for: picked)
        overview = o
        // The old curve stays until the new one is drawn, so the page never
        // empties between presets.
        if let device = o.device {
            response = await app.dsp.outputResponse(for: device)
        }
        var drawn: [String: [Double]] = [:]
        for name in [o.active].compactMap({ $0 }) + o.chain.map(\.name) {
            drawn[name] = await app.dsp.response(name)?.total
        }
        curves = drawn
        if let active = o.active, let targets = await app.dsp.targets(active) {
            let id = targets.chosen ?? targets.madeFor?.id
            aim = targets.choices.first { $0.id == id }?.name
        } else {
            aim = nil
        }
    }
}

/// A stage's place in the chain, to choose what fills it.
enum Stage: String, Identifiable {
    case correction, eq
    var id: Self { self }
}

/// How a stage is drawn, the same in its block and on the graph: the
/// correction in the accent, each EQ in ink with a dash of its own, the
/// whole chain in the strongest ink.
struct StageStroke {
    let style: AnyShapeStyle
    let dash: [CGFloat]

    static let correction = StageStroke(style: AnyShapeStyle(.tint), dash: [])
    static let total = StageStroke(style: KoanTheme.style(.strong, system: Color.primary), dash: []) // theme: raw — the system look's own

    static func eq(_ index: Int) -> StageStroke {
        let dashes: [[CGFloat]] = [[5, 3], [2, 2], [8, 3, 2, 3], [1, 4]]
        let system: [Color] = [.orange, .teal, .pink, .indigo] // theme: raw — the system look's series
        return StageStroke(
            style: KoanTheme.style(.ink, system: system[index % system.count]),
            dash: dashes[index % dashes.count]
        )
    }
}

/// A curve as a small line, for a stage's block.
struct CurveThumb: View {
    let db: [Double]
    let stroke: StageStroke

    var body: some View {
        Canvas { context, size in
            guard db.count > 1 else { return }
            let range = max(6, db.map(abs).max() ?? 0)
            var path = Path()
            for (i, v) in db.enumerated() {
                let point = CGPoint(
                    x: size.width * CGFloat(i) / CGFloat(db.count - 1),
                    y: size.height / 2 - size.height / 2 * CGFloat(v / range)
                )
                if i == 0 { path.move(to: point) } else { path.addLine(to: point) }
            }
            context.stroke(path, with: .style(stroke.style), style: StrokeStyle(lineWidth: 1.5, dash: stroke.dash))
        }
        .frame(width: 64, height: 26)
        .accessibilityHidden(true)
    }
}

/// The chain as blocks joined by a line: music in, the correction, the
/// tuning's EQs, the device out. An empty stage is a dashed place to add one.
struct EqChain: View {
    let overview: DspOverview
    let device: String
    let aim: String?
    let curves: [String: [Double]]
    let choose: (Stage) -> Void
    let open: (String) -> Void
    let set: ([DspTuningEntry]) -> Void

    private var correction: DspProfileSummary? {
        overview.profiles.first { $0.name == overview.active }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            end(KoanTheme.label("Music in"), systemImage: "music.note")
            link
            if let correction {
                StageBlock(
                    title: "Correction",
                    name: correction.name,
                    detail: correction.role == .baked ? KoanTheme.label("Already includes a tuning") : DspModel.describe(correction),
                    db: curves[correction.name],
                    stroke: .correction,
                    action: { choose(.correction) }
                ) {
                    #if !os(tvOS)
                    Button("Show") { open(correction.name) }
                        .koanButton(.text)
                    #endif
                }
            } else {
                Placeholder(title: "Correction", prompt: "Add a correction", action: { choose(.correction) })
            }
            link
            Text("Tuning")
                .koanText(.fine, .muted)
                .koanCase()
                .padding(.vertical, 4)
            ForEach(Array(overview.chain.enumerated()), id: \.element.name) { i, entry in
                StageBlock(
                    title: "EQ \(i + 1)",
                    name: entry.name,
                    detail: entry.on ? nil : KoanTheme.label("Off"),
                    db: curves[entry.name],
                    stroke: .eq(i),
                    action: { open(entry.name) }
                ) {
                    #if !os(tvOS)
                    Toggle("On", isOn: Binding(
                        get: { entry.on },
                        set: { on in set(overview.chain.enumerated().map { $0 == i ? DspTuningEntry(name: $1.name, on: on) : $1 }) }
                    ))
                    .koanToggle()
                    .fixedSize()
                    Menu("Options") {
                        Button("Edit") { open(entry.name) }
                        if i > 0 {
                            Button("Move Up") { move(i, by: -1) }
                        }
                        if i < overview.chain.count - 1 {
                            Button("Move Down") { move(i, by: 1) }
                        }
                        Button("Remove from Tuning", role: .destructive) {
                            set(overview.chain.enumerated().filter { $0.offset != i }.map(\.element))
                        }
                    }
                    .koanControl()
                    .fixedSize()
                    #endif
                }
                link
            }
            #if !os(tvOS)
            if correction?.role != .baked {
                Placeholder(title: nil, prompt: "Add EQ", action: { choose(.eq) })
                link
            }
            #endif
            end("\(device) out", systemImage: "hifispeaker")
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel(Self.sentence(overview, device: device, aim: aim))
    }

    private func move(_ i: Int, by step: Int) {
        var chain = overview.chain
        chain.swapAt(i, i + step)
        set(chain)
    }

    private var link: some View {
        Rectangle()
            .fill(KoanTheme.style(.rule, system: Color.secondary.opacity(0.5))) // theme: raw — the system look's own
            .frame(width: KoanTheme.hairline, height: 14)
            .padding(.leading, 18)
            .accessibilityHidden(true)
    }

    private func end(_ title: String, systemImage: String) -> some View {
        Label(title, systemImage: systemImage)
            .koanText(.meta, .muted)
            .padding(.leading, 8)
    }

    /// The chain in words, for the page's footer and for VoiceOver.
    /// What plays: the EQs switched on and not left out, and none on a
    /// correction that already includes a tuning.
    static func sentence(_ o: DspOverview, device: String, aim: String?) -> String {
        let includes = o.profiles.first { $0.name == o.active }?.role == .baked
        let eqs = includes ? [] : o.chain.filter { entry in
            entry.on && !o.leftOutEqs.contains(entry.name)
        }.map(\.name)
        guard o.active != nil || !eqs.isEmpty else {
            return "\(device) is flat: the music plays untouched."
        }
        var parts: [String] = []
        if let active = o.active {
            parts.append(aim.map { "corrected by \(active) to \($0)" } ?? "corrected by \(active)")
        }
        if !eqs.isEmpty {
            parts.append("tuned with " + ListFormatter.localizedString(byJoining: eqs))
        }
        return "Music to \(device), " + parts.joined(separator: ", then ") + "."
    }
}

/// A stage of the chain: what it is, what fills it, its curve, and its
/// controls.
private struct StageBlock<Controls: View>: View {
    let title: String
    let name: String
    let detail: String?
    let db: [Double]?
    let stroke: StageStroke
    let action: () -> Void
    @ViewBuilder let controls: () -> Controls

    var body: some View {
        HStack(spacing: KoanTheme.Space.m) {
            Button(action: action) {
                HStack(spacing: KoanTheme.Space.m) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(title)
                            .koanText(.fine, .muted)
                            .koanCase()
                        Text(name)
                            .koanText(.body)
                            .lineLimit(1)
                        if let detail, !detail.isEmpty {
                            Text(detail)
                                .koanText(.fine, .muted)
                                .lineLimit(1)
                        }
                    }
                    Spacer(minLength: 0)
                    if let db {
                        CurveThumb(db: db, stroke: stroke)
                    }
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("\(title): \(name)" + (detail.map { ", \($0)" } ?? ""))
            controls()
        }
        .padding(KoanTheme.Space.s)
        .overlay(Rectangle().stroke(KoanTheme.style(.rule, system: Color.secondary.opacity(0.4)), lineWidth: KoanTheme.hairline)) // theme: raw — the system look's own
    }
}

/// An empty stage: a dashed place to choose what fills it.
private struct Placeholder: View {
    let title: String?
    let prompt: String
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            VStack(alignment: .leading, spacing: 2) {
                if let title {
                    Text(title).koanText(.fine, .muted).koanCase()
                }
                Label(prompt, systemImage: "plus")
                    .koanText(.body, .accent)
                    .koanCase()
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(KoanTheme.Space.s)
            .overlay(Rectangle().stroke(KoanTheme.style(.muted, system: Color.secondary), style: StrokeStyle(lineWidth: 1, dash: [4, 3]))) // theme: raw — the system look's own
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        #if os(tvOS)
        .disabled(true)
        #endif
    }
}

/// Where an Add… in a stage's picker leads, presented by the page.
enum StageAdd {
    case importing, autoEq, measuring
    case splitting(String)
}

/// What fills a stage, chosen: a correction, with the target it aims at and
/// a group's member, or an EQ to add to the tuning. Add… at the foot brings
/// in another.
struct StagePicker: View {
    let dsp: DspModel
    let stage: Stage
    let overview: DspOverview
    let device: String
    let add: (StageAdd) -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var targets: DspTargets?

    private var correction: DspProfileSummary? {
        overview.profiles.first { $0.name == overview.active }
    }

    private var choices: [DspProfileSummary] {
        switch stage {
        case .correction:
            overview.profiles.filter { !$0.preset && $0.role != .tuning }
        case .eq:
            overview.profiles.filter { p in
                !p.preset && p.role == .tuning && p.rates.isEmpty && !overview.chain.contains { $0.name == p.name }
            }
        }
    }

    var body: some View {
        NavigationStack {
            KoanForm {
                Section {
                    if stage == .correction {
                        row("None", detail: "No correction: the device as it is", chosen: overview.active == nil) {
                            dsp.assign(nil, to: device)
                        }
                    }
                    ForEach(choices, id: \.name) { p in
                        row(p.name, detail: DspModel.describe(p), chosen: stage == .correction && overview.active == p.name) {
                            switch stage {
                            case .correction: dsp.assign(p.name, to: device)
                            case .eq: dsp.setTunings(overview.chain + [DspTuningEntry(name: p.name, on: true)], for: device)
                            }
                        }
                    }
                    if choices.isEmpty, stage == .eq {
                        Text("Every EQ is in the tuning already. Add another below.")
                            .koanText(.meta, .muted)
                    }
                } header: {
                    KoanSectionHeader(stage == .correction ? "Corrections" : "EQs")
                }
                if stage == .correction, let name = overview.active {
                    current(name)
                }
                Section {
                    Button("Import a File…") { add(.importing) }
                        .koanButton(.standard)
                    Button("Find in AutoEQ…") { add(.autoEq) }
                        .koanButton(.standard)
                    Button("Find a Measurement…") { add(.measuring) }
                        .koanButton(.standard)
                } header: {
                    KoanSectionHeader("Add…")
                }
            }
            .navigationTitle(KoanTheme.label(stage == .correction ? "Correction" : "Add EQ"))
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
            .task(id: "\(overview.active ?? "")\u{0}\(dsp.stamp)") {
                targets = if let name = overview.active { await dsp.targets(name) } else { nil }
            }
        }
        #if os(macOS)
        .frame(minWidth: 420, minHeight: 460)
        #endif
    }

    /// The correction chosen: the target it aims at, a group's member, or
    /// for one with a tuning baked in, the way to take it apart.
    @ViewBuilder private func current(_ name: String) -> some View {
        Section {
            if let targets, correction?.role == .correction {
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
            if let c = correction, !c.members.isEmpty {
                KoanPicker(
                    "Playing",
                    selection: Binding(
                        get: { c.playing ?? c.members.first ?? "" },
                        set: { dsp.select(name, $0) }
                    ),
                    options: c.members.map { ($0, $0) },
                    keepsCase: true
                )
            }
            if let c = correction, c.role == .baked {
                Label("\(name) already includes a tuning, so no other tuning plays on it. Split it into a correction and a tuning to change that.", systemImage: "info.circle")
                    .koanText(.meta, .muted)
                if c.rates.isEmpty, c.layers == 0 {
                    Button("Split into Correction + Tuning…") { add(.splitting(name)) }
                        .koanButton(.compact)
                }
            }
        } header: {
            KoanSectionHeader(name)
        }
    }

    private func row(_ name: String, detail: String, chosen: Bool, choose: @escaping () -> Void) -> some View {
        Button {
            choose()
            dismiss()
        } label: {
            HStack {
                VStack(alignment: .leading, spacing: 2) {
                    Text(name)
                    if !detail.isEmpty {
                        Text(detail).koanText(.fine, .muted)
                    }
                }
                Spacer()
                if chosen {
                    Image(systemName: "checkmark")
                        .koanText(.body, .accent)
                        .accessibilityLabel("Chosen")
                }
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

/// What the words on the EQ page mean.
struct EqExplainer: View {
    @Environment(\.dismiss) private var dismiss

    private let words: [(String, String)] = [
        ("Correction", "Makes your device neutral: headphones or speakers, measured and brought to a target. A device has one, and it plays as it was made, so it is not edited here; a tuning goes on top."),
        ("Tuning", "Your taste on top of the correction: one or more EQs, played in order, each switched on or off."),
        ("EQ", "One set of bands. Open it from the chain to edit it."),
        ("Preset", "A correction and tuning saved together, to switch a device between, or set another device from."),
        ("Flat", "Nothing chosen: the music plays untouched, bit for bit."),
    ]

    var body: some View {
        NavigationStack {
            KoanForm {
                Section {
                    Text("Music plays through the correction, then the tuning's EQs in order, then out to the device. Each device has its own.")
                        .koanText(.body)
                }
                Section {
                    ForEach(words, id: \.0) { word, meaning in
                        VStack(alignment: .leading, spacing: 4) {
                            Text(word).koanText(.body, .strong)
                            Text(meaning).koanText(.meta, .muted)
                        }
                        .accessibilityElement(children: .combine)
                    }
                } header: {
                    KoanSectionHeader("Words")
                }
                Section {
                    Text("An EQ made against one target and played on a correction aiming at another gets the difference between the two added, so it sounds as it was made.")
                        .koanText(.meta, .muted)
                }
            }
            .navigationTitle(KoanTheme.label("How EQ works"))
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
        #if os(macOS)
        .frame(minWidth: 420, minHeight: 460)
        #endif
    }
}
