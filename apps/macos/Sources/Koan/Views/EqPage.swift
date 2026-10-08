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
                        curves: curves,
                        choose: { choosing = $0 },
                        open: { showing = ShownProfile(name: $0) },
                        set: { app.dsp.setTunings($0, for: device) },
                        madeFor: { app.dsp.setTunedFor($0, $1) }
                    )
                } footer: {
                    Text(EqChain.sentence(o, device: app.dsp.label(device)))
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
                    .koanButton(.link)
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
                        KoanSheetAction(placement: .confirmationAction) {
                            Button("Done") { managing = false }
                        }
                    }
            }
            .frame(minWidth: 480, minHeight: 520)
            .koanSheet()
        }
        #else
        .navigationDestination(isPresented: $managing) {
            ManageEq(device: device, active: overview?.active, chain: overview?.chain ?? [])
                .koanPushedPage()
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
        .formTray(item: $finding) { find in
            AutoEqSearch(dsp: app.dsp, query: find.query)
        }
        .formTray(isPresented: $measuring) {
            MeasurementFlow(dsp: app.dsp)
        }
        .formTray(item: $splitting) { baked in
            SplitFlow(dsp: app.dsp, name: baked.name)
        }
        #endif
        // On a television, the account's own profiles only: no file reaches
        // one, and it has no microphone to measure with.
        .formTray(item: $choosing, onDismiss: {
            #if !os(tvOS)
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
            #endif
        }) { stage in
            if let o = overview, let device {
                StagePicker(dsp: app.dsp, stage: stage, overview: o, device: device) { add in
                    adding = (stage, add)
                    choosing = nil
                }
            }
        }
        #if !os(tvOS)
        .formTray(isPresented: $explaining) {
            EqExplainer()
        }
        // A profile imported from a file: a neutral correction, one with a
        // tuning already in it, or taste to add on top? kōan cannot tell,
        // and a chain corrects once.
        .formTray(item: Binding(
            // Manage EQ asks for its own imports while it is open.
            get: { managing ? nil : app.dsp.askRole },
            // Swiped away, as Decide Later.
            set: { if $0 == nil, let ask = app.dsp.askRole { app.dsp.answer(ask, nil) } }
        )) { ask in
            RoleQuestion(dsp: app.dsp, ask: ask)
        }
        .alert(namingTitle, isPresented: $naming) {
            TextField("Name", text: $presetName)
            Button("Save") { save(as: presetName, over: false) }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("The correction and tuning, to switch \(device.map(app.dsp.label) ?? "a device") back to, or another device to.")
        }
        #endif
        #if os(iOS) || os(tvOS)
        .navigationDestination(item: $showing) { shown in
            DspProfilePage(dsp: app.dsp, name: shown.name, device: device)
                .koanPushedPage()
        }
        #elseif os(macOS)
        .sheet(item: $showing) { shown in
            NavigationStack {
                DspProfilePage(dsp: app.dsp, name: shown.name, device: device)
                    .toolbar {
                        KoanSheetAction(placement: .confirmationAction) {
                            Button("Done") { showing = nil }
                        }
                    }
            }
            .koanSheetStack()
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
                            .koanButton(.bordered)
                        Button("Revert") { app.dsp.applyPreset(preset, to: device) }
                            .koanButton(.link)
                        Button("Save as New…") { ask("Save as New Preset") }
                            .koanButton(.link)
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
                .koanButton(.link)
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
    /// The dB at the top edge, shared where thumbnails are compared; the
    /// curve's own peak, at least 6 dB, otherwise.
    var range: Double?

    var body: some View {
        Canvas { context, size in
            guard db.count > 1 else { return }
            let range = self.range ?? max(6, db.map(abs).max() ?? 0)
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
/// Above each EQ, how it meets the correction: the line in the accent where
/// it was made against the correction's target, the conversion koan plays
/// where it was made against another, and a warning where that is not set,
/// since then a target may be applied twice.
struct EqChain: View {
    let overview: DspOverview
    let device: String
    let curves: [String: [Double]]
    let choose: (Stage) -> Void
    let open: (String) -> Void
    let set: ([DspTuningEntry]) -> Void
    /// Say what an EQ was made against: its name, a target's id.
    let madeFor: (String, String) -> Void

    private var correction: DspProfileSummary? {
        overview.profiles.first { $0.name == overview.active }
    }

    /// Every EQ that plays meets the correction matched: the whole chain is
    /// drawn in the accent, as the line into a matched EQ is.
    private var matched: Bool {
        guard correction != nil else { return false }
        let playing = zip(overview.chain, overview.joins).filter { $0.0.on }
        return !playing.isEmpty && playing.allSatisfy { $0.1.join == .matched }
    }

    /// The chain's lines and outlines: the accent when it is matched.
    private var outline: AnyShapeStyle {
        matched ? AnyShapeStyle(.tint) : KoanTheme.style(.rule, system: Color.secondary.opacity(0.5)) // theme: raw — the system look's own
    }

    var body: some View {
        #if os(iOS)
        // Each EQ a row of the list, for its swipe actions; the rows meet,
        // so the line through the chain runs unbroken.
        Group {
            head
            ForEach(Array(overview.chain.enumerated()), id: \.element.name) { i, entry in
                eq(i, entry)
                    .swipeActions(edge: .trailing) {
                        Button("Remove", role: .destructive) { remove(i) }
                    }
                    .swipeActions(edge: .leading) {
                        if i > 0 {
                            Button("Move Up") { move(i, by: -1) }
                        }
                        if i < overview.chain.count - 1 {
                            Button("Move Down") { move(i, by: 1) }
                        }
                    }
                    .contextMenu {
                        if i > 0 {
                            Button("Move Up", systemImage: "arrow.up") { move(i, by: -1) }
                        }
                        if i < overview.chain.count - 1 {
                            Button("Move Down", systemImage: "arrow.down") { move(i, by: 1) }
                        }
                        Button("Remove from Tuning", systemImage: "trash", role: .destructive) { remove(i) }
                    }
            }
            tail
        }
        .listRowInsets(.vertical, 0)
        #else
        VStack(alignment: .leading, spacing: 0) {
            head
            ForEach(Array(overview.chain.enumerated()), id: \.element.name) { i, entry in
                eq(i, entry)
            }
            tail
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel(Self.sentence(overview, device: device))
        #endif
    }

    /// Music in, the correction, and the tuning's heading.
    private var head: some View {
        VStack(alignment: .leading, spacing: 0) {
            end(KoanTheme.label("Music in"), systemImage: "music.note")
            link
            if let correction {
                StageBlock(
                    title: "Correction",
                    name: correction.name,
                    detail: correction.role == .baked
                        ? KoanTheme.label("Already includes a tuning")
                        : ([DspModel.describe(correction)] + [overview.aim.map { "to \($0)" }].compactMap { $0 })
                            .filter { !$0.isEmpty }
                            .joined(separator: " · "),
                    db: curves[correction.name],
                    stroke: .correction,
                    outline: outline,
                    action: { choose(.correction) }
                ) {
                    #if !os(tvOS)
                    Button("Show") { open(correction.name) }
                        .koanButton(.link)
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
        }
    }

    /// One of the tuning's EQs, and the line into it. Tapped, it opens the
    /// EQ's page. On a phone it is moved and removed by swiping; elsewhere
    /// from its menu.
    private func eq(_ i: Int, _ entry: DspTuningEntry) -> some View {
        let meets = overview.joins.indices.contains(i) ? overview.joins[i] : nil
        return VStack(alignment: .leading, spacing: 0) {
            if i > 0 || meets?.join != nil || meets?.note != nil {
                join(meets, eq: entry.name)
            }
            StageBlock(
                title: "EQ \(i + 1)",
                name: entry.name,
                detail: entry.on ? meets?.madeFor.map { "made for \($0)" } : KoanTheme.label("Off"),
                db: curves[entry.name],
                stroke: .eq(i),
                outline: outline,
                action: { open(entry.name) }
            ) {
                #if !os(tvOS)
                Toggle("On", isOn: Binding(
                    get: { entry.on },
                    set: { on in set(overview.chain.enumerated().map { $0 == i ? DspTuningEntry(name: $1.name, on: on) : $1 }) }
                ))
                .koanToggle()
                .fixedSize()
                #endif
                #if os(macOS)
                Menu("Options") {
                    Button("Edit") { open(entry.name) }
                    if i > 0 {
                        Button("Move Up") { move(i, by: -1) }
                    }
                    if i < overview.chain.count - 1 {
                        Button("Move Down") { move(i, by: 1) }
                    }
                    Button("Remove from Tuning", role: .destructive) { remove(i) }
                }
                .koanControl()
                .fixedSize()
                #endif
            }
        }
    }

    /// Adding an EQ, and the device out.
    private var tail: some View {
        VStack(alignment: .leading, spacing: 0) {
            if !overview.chain.isEmpty {
                link
            }
            #if !os(tvOS)
            if correction?.role != .baked {
                Placeholder(title: nil, prompt: "Add EQ", outline: matched ? AnyShapeStyle(.tint) : nil, action: { choose(.eq) })
                link
            }
            #endif
            end("\(device) out", systemImage: "hifispeaker")
        }
    }

    private func remove(_ i: Int) {
        set(overview.chain.enumerated().filter { $0.offset != i }.map(\.element))
    }

    private func move(_ i: Int, by step: Int) {
        var chain = overview.chain
        chain.swapAt(i, i + step)
        set(chain)
    }

    private var link: some View {
        Rectangle()
            .fill(outline)
            .frame(width: matched ? 2 : KoanTheme.hairline, height: 14)
            .padding(.leading, 18)
            .accessibilityHidden(true)
    }

    /// The line into an EQ, saying how it meets the correction, and what of
    /// it does not play as chosen.
    private func join(_ meets: DspEqJoin?, eq: String) -> some View {
        let matched = meets?.join == .matched || self.matched
        return VStack(alignment: .leading, spacing: 2) {
            switch meets?.join {
            case .matched:
                Label("Matched", systemImage: "checkmark")
                    .koanText(.fine, .accent)
                    .koanCase()
            case let .converted(from, to):
                HStack(spacing: KoanTheme.Space.m) {
                    Text("Target difference: \(from) → \(to)")
                        .koanText(.fine, .muted)
                    if let step = meets?.step, step.count > 1 {
                        CurveThumb(db: step, stroke: StageStroke(style: KoanTheme.style(.muted, system: Color.secondary), dash: [])) // theme: raw — the system look's own
                    }
                }
            case let .refitted(_, to):
                Text("Correction fitted to \(to) for it")
                    .koanText(.fine, .muted)
            case .unknown:
                #if os(tvOS)
                Label("Made against: unknown. This may apply a target twice", systemImage: "exclamationmark.triangle")
                    .koanText(.fine, .bad)
                #else
                Button { open(eq) } label: {
                    Label("Made against: unknown. This may apply a target twice; set it", systemImage: "exclamationmark.triangle")
                        .koanText(.fine, .bad)
                        .multilineTextAlignment(.leading)
                }
                .buttonStyle(.plain)
                if let suggestion = meets?.suggestion {
                    Button("Looks made for \(suggestion.name). Use that?") { madeFor(eq, suggestion.id) }
                        .koanButton(.link)
                }
                #endif
            case nil:
                EmptyView()
            }
            if let note = meets?.note {
                Label(note, systemImage: "exclamationmark.triangle")
                    .koanText(.fine, .bad)
            }
        }
        .padding(.vertical, 4)
        .frame(minHeight: 14, alignment: .leading)
        .padding(.leading, 18 + KoanTheme.Space.m)
        .background(alignment: .leading) {
            Rectangle()
                .fill(matched ? AnyShapeStyle(.tint) : outline)
                .frame(width: matched ? 2 : KoanTheme.hairline)
                .padding(.leading, 18)
                .accessibilityHidden(true)
        }
    }

    private func end(_ title: String, systemImage: String) -> some View {
        Label(title, systemImage: systemImage)
            .koanText(.meta, .muted)
            .padding(.leading, 8)
    }

    /// The chain in words, for the page's footer and for VoiceOver.
    /// What plays: the EQs switched on and not left out, and none on a
    /// correction that already includes a tuning.
    static func sentence(_ o: DspOverview, device: String) -> String {
        let aim = o.aim
        let includes = o.profiles.first { $0.name == o.active }?.role == .baked
        let eqs = includes ? [] : o.chain.filter { entry in
            entry.on && !o.leftOutEqs.contains(entry.name)
        }.map(\.name)
        // As core's `joined` says each, for the CLI.
        let meets: [String] = zip(o.chain, o.joins).compactMap { entry, meets in
            guard eqs.contains(entry.name), let aim else { return nil }
            switch meets.join {
            case .matched: return "\(entry.name) was made for \(aim): matched."
            case let .converted(from, to): return "\(entry.name) was made for \(to), so the difference from \(from) to \(to) plays first."
            case let .refitted(_, to): return "\(entry.name) was made for \(to), so the correction is fitted to \(to) for it."
            case .unknown: return "What \(entry.name) was made against is not set, so it may apply a target twice."
            case nil: return nil
            }
        }
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
        return (["Music to \(device), " + parts.joined(separator: ", then ") + "."] + meets).joined(separator: " ")
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
    let outline: AnyShapeStyle
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
        .overlay(Rectangle().stroke(outline, lineWidth: KoanTheme.hairline))
    }
}

/// An empty stage: a dashed place to choose what fills it.
private struct Placeholder: View {
    let title: String?
    let prompt: String
    /// In place of the dashed `muted` outline: the accent of a matched chain.
    var outline: AnyShapeStyle?
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
            .overlay(Rectangle().stroke(outline ?? KoanTheme.style(.muted, system: Color.secondary), style: StrokeStyle(lineWidth: 1, dash: [4, 3]))) // theme: raw — the system look's own
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

/// A target among a correction's choices, its default marked.
private struct TargetChoiceRow: View {
    let targets: DspTargets
    let id: String

    var body: some View {
        if let c = targets.choices.first(where: { $0.id == id }) {
            TargetRow(target: c, isDefault: c.id == targets.madeFor?.id)
        }
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

    /// The EQs made against the target the correction aims at, ahead of the
    /// rest, each alphabetical; one list where there is no correction to
    /// match.
    private var eqGroups: [(title: String, eqs: [DspProfileSummary])] {
        guard !choices.isEmpty else { return [] }
        guard overview.aim != nil else { return [(title: "EQs", eqs: choices)] }
        let sorted = choices.sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
        let matching = sorted.filter { $0.join == .matched }
        let other = sorted.filter { $0.join != .matched }
        guard !matching.isEmpty else { return [(title: "EQs", eqs: other)] }
        return [(title: "Matches your correction", eqs: matching)] + (other.isEmpty ? [] : [(title: "Other", eqs: other)])
    }

    /// Nothing left to add to the tuning. A television takes profiles from
    /// the account's other devices.
    private var emptyEqs: String {
        let none = overview.profiles.allSatisfy { $0.preset || $0.role != .tuning || !$0.rates.isEmpty }
        #if os(tvOS)
        return none
            ? "No EQs yet. Add them on your phone or Mac, and they appear here."
            : "Every EQ is in the tuning already. Add more on your phone or Mac."
        #else
        return "Every EQ is in the tuning already. Add another below."
        #endif
    }

    var body: some View {
        NavigationStack {
            KoanForm {
                switch stage {
                case .correction:
                    Section {
                        row("None", detail: "No correction: the device as it is", chosen: overview.active == nil) {
                            dsp.assign(nil, to: device)
                        }
                        ForEach(choices, id: \.name) { p in
                            row(p.name, detail: DspModel.describe(p), chosen: overview.active == p.name) {
                                dsp.assign(p.name, to: device)
                            }
                        }
                    } header: {
                        KoanSectionHeader("Corrections")
                    } footer: {
                        #if os(tvOS)
                        if choices.isEmpty {
                            Text("No corrections yet. Add them on your phone or Mac, and they appear here.")
                                .koanText(.meta, .muted)
                        }
                        #endif
                    }
                case .eq:
                    if choices.isEmpty {
                        Section {
                            Text(emptyEqs)
                                .koanText(.meta, .muted)
                        } header: {
                            KoanSectionHeader("EQs")
                        }
                    }
                    ForEach(eqGroups, id: \.title) { group in
                        Section {
                            ForEach(group.eqs, id: \.name) { p in
                                row(p.name, detail: DspModel.describe(p), madeFor: p.madeFor, matched: p.join == .matched, chosen: false) {
                                    dsp.setTunings(overview.chain + [DspTuningEntry(name: p.name, on: true)], for: device)
                                }
                            }
                        } header: {
                            KoanSectionHeader(group.title)
                        }
                    }
                }
                if stage == .correction, let name = overview.active {
                    current(name)
                }
                #if !os(tvOS)
                Section {
                    Button("Import a File…") { add(.importing) }
                        .koanButton(.bordered)
                    Button("Find in AutoEQ…") { add(.autoEq) }
                        .koanButton(.bordered)
                    Button("Find a Measurement…") { add(.measuring) }
                        .koanButton(.bordered)
                } header: {
                    KoanSectionHeader("Add…")
                }
                #endif
            }
            .navigationTitle(KoanTheme.label(stage == .correction ? "Correction" : "Add EQ"))
            .toolbar {
                KoanSheetAction(placement: .confirmationAction) {
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

    private func targetPicker(_ name: String, _ targets: DspTargets) -> some View {
        let selection = Binding(
            get: { targets.chosen ?? targets.madeFor?.id ?? "" },
            set: { (id: String) in dsp.chooseTarget(name, id == targets.madeFor?.id ? nil : id) }
        )
        let ids: [String] = targets.choices.map(\.id)
        return KoanListPicker(
            title: "Target",
            selection: selection,
            sections: [(title: nil, values: ids)],
            name: { (id: String) -> String in targets.choices.first { $0.id == id }?.name ?? "" },
            row: { (id: String) in TargetChoiceRow(targets: targets, id: id) }
        )
    }

    /// The correction chosen: the target it aims at, a group's member, or
    /// for one with a tuning baked in, the way to take it apart.
    @ViewBuilder private func current(_ name: String) -> some View {
        Section {
            if let targets, correction?.role == .correction {
                targetPicker(name, targets)
                    .koanControl()
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
                #if !os(tvOS)
                if c.rates.isEmpty, c.layers == 0 {
                    Button("Split into Correction + Tuning…") { add(.splitting(name)) }
                        .koanButton(.compact)
                }
                #endif
            }
        } header: {
            KoanSectionHeader(name)
        }
    }

    /// A choice: its name, for an EQ the target it was made against, in the
    /// accent where that is the correction's, and what it holds.
    private func row(
        _ name: String,
        detail: String,
        madeFor: String? = nil,
        matched: Bool = false,
        chosen: Bool,
        choose: @escaping () -> Void
    ) -> some View {
        Button {
            choose()
            dismiss()
        } label: {
            HStack {
                VStack(alignment: .leading, spacing: 2) {
                    Text(name)
                    if let madeFor {
                        Text(madeFor)
                            .textCase(nil)
                            .koanBadge(accent: matched)
                            .padding(.vertical, 2)
                            .accessibilityLabel("Made against \(madeFor)")
                    }
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
                KoanSheetAction(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
        #if os(macOS)
        .frame(minWidth: 420, minHeight: 460)
        #endif
    }
}
