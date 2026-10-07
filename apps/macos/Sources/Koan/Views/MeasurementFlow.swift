#if !os(tvOS)
import KoanFFI
import SwiftUI
import UniformTypeIdentifiers

/// Correcting headphones AutoEQ does not have, from a measurement of them:
/// what a measurement is and where to get one, the file, in-ear or
/// over-ear, a target, and what the correction does before it is saved.
/// Every word it uses is explained once, where it first appears.
struct MeasurementFlow: View {
    let dsp: DspModel
    /// Called once the profile is saved, with its name.
    var saved: (String) -> Void = { _ in }
    @Environment(\.dismiss) private var dismiss

    @State private var step = Step.learn
    @State private var text = ""
    @State private var file: String?
    @State private var choosing = false
    @State private var inEar = true
    @State private var targets: [DspTargetOption] = []
    @State private var target: String?
    @State private var preview: DspResponse?
    @State private var name: String
    @State private var problem: String?
    /// What the measurement was read as: a speaker's or a headphone's, and
    /// for a speaker which of its curves.
    @State private var reading: DspMeasurementReading?
    @State private var saving = false
    /// Searching squig.link's sites, from the name the flow was opened with.
    @State private var squigQuery: String
    @State private var hits: [SquigHit] = []
    @State private var searching = false
    /// Why a search shows nothing: no match, or no site reached.
    @State private var searchNote: String?
    /// The result being fetched.
    @State private var picking: SquigHit?
    /// The result the measurement came from.
    @State private var fetched: SquigHit?
    /// Why the last result tapped could not be fetched, shown under it.
    @State private var fetchProblem: (hit: SquigHit, message: String)?
    /// Where the measurement came from, credited on the correction.
    @State private var source: String?

    init(dsp: DspModel, name: String = "", saved: @escaping (String) -> Void = { _ in }) {
        self.dsp = dsp
        self.saved = saved
        _name = State(initialValue: name)
        _squigQuery = State(initialValue: name)
    }

    enum Step: Int, CaseIterable {
        case learn, file, ear, target, review

        var title: String {
            switch self {
            case .learn: "Find a measurement"
            case .file: "Your measurement"
            case .ear: "In-ear or over-ear"
            case .target: "Choose a target"
            case .review: "Check and save"
            }
        }
    }

    var body: some View {
        NavigationStack {
            ScrollViewReader { scroller in
                KoanForm {
                    switch step {
                    case .learn: learn
                    case .file: fileStep
                    case .ear: ear
                    case .target: targetStep
                    case .review: review
                    }
                    if let problem {
                        Section {
                            Label(problem, systemImage: "exclamationmark.triangle.fill")
                                .foregroundStyle(KoanTheme.style(.bad, system: .orange))
                        }
                        .id(Self.problemID)
                    }
                }
                // The file step is long: a problem said below the fold would
                // leave Next looking as if it did nothing.
                .onChange(of: problem) { _, problem in
                    guard problem != nil else { return }
                    withAnimation { scroller.scrollTo(Self.problemID, anchor: .bottom) }
                }
            }
            .navigationTitle(KoanTheme.label(step.title))
            .toolbar {
                // Cancel and Back as one item of text: two items share one
                // pane of glass on iOS, too narrow for both words.
                ToolbarItem(placement: .cancellationAction) {
                    HStack(spacing: 16) {
                        Button("Cancel") { dismiss() }
                        if step != .learn {
                            Button("Back") {
                                problem = nil
                                // A speaker has no in-ear or over-ear to ask.
                                step = step == .target && speaker
                                    ? .file
                                    : Step(rawValue: step.rawValue - 1) ?? .learn
                            }
                        }
                    }
                    .koanButtons(.text)
                    .fixedSize()
                }
                .sharedBackgroundVisibility(KoanTheme.pane(.automatic))
                ToolbarItem(placement: .confirmationAction) {
                    Group {
                        if step == .review {
                            Button("Save") { save() }
                                .disabled(name.trimmingCharacters(in: .whitespaces).isEmpty || saving)
                        } else {
                            Button("Next") { Task { await next() } }
                                .disabled(!canGoOn)
                        }
                    }
                    .koanButtons(.text)
                    .fixedSize()
                }
                .sharedBackgroundVisibility(KoanTheme.pane(.automatic))
            }
            .fileImporter(
                isPresented: $choosing,
                allowedContentTypes: [.commaSeparatedText, .plainText, .text, .data],
                allowsMultipleSelection: true
            ) { result in
                if case let .success(urls) = result, !urls.isEmpty { read(urls) }
            }
        }
        #if os(macOS)
        .frame(minWidth: 520, minHeight: 560)
        #endif
    }

    private static let problemID = "problem"

    // MARK: - Steps

    private var learn: some View {
        Group {
            Section {
                Text("A **measurement** is a graph of how loud your headphones play each pitch, from deep bass to high treble. People who test headphones publish them.")
                Text("You choose a **target**, the sound you want. kōan works out a **correction**: what to turn up and down so your headphones sound like the target.")
            }
            Section("Where to get one") {
                Link("squig.link", destination: URL(string: "https://squig.link")!)
                Text("Search for your headphones on the next page: kōan looks through the reviewers' squig.link sites. Or save their frequency response as a file from one.")
                Text("REW and most measurement tools export one too.")
            }
            Section {
                Text("20, 92.4\n21, 92.6\n22, 92.7\n…")
                    .font(.system(.body, design: .monospaced))
                    .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
            } header: {
                Text("What the file looks like")
            } footer: {
                Text("Two numbers a line: a frequency in hertz, then a level in decibels. Commas, tabs or spaces between them all work.")
                    .koanText(.fine, .muted)
            }
            Section {
                Link("Learn more about headphone EQ", destination: URL(string: "https://koan.rocks/docs/headphone-eq/")!)
            }
        }
    }

    private var fileStep: some View {
        Group {
            Section {
                TextField("Search…", text: $squigQuery)
                    .task(id: squigQuery) { await search() }
                if searching, hits.isEmpty {
                    HStack(spacing: 8) {
                        ProgressView().controlSize(.small)
                        Text("Searching squig.link…").koanText(.meta, .muted)
                    }
                } else if let searchNote {
                    Text(searchNote).koanText(.meta, .muted)
                }
                ForEach(hits.prefix(20), id: \.self) { hit in
                    if hit.locked != nil, let site = URL(string: hit.site) {
                        // Fetching would only fail: the site is offered instead.
                        Link(destination: site) {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(hit.name)
                                Text(details(hit) + " · opens in a browser")
                                    .koanText(.fine, .muted)
                            }
                            .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                    } else {
                        Button { pick(hit) } label: {
                            HStack {
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(hit.name)
                                    Text(details(hit)).koanText(.fine, .muted)
                                    if let fetchProblem, fetchProblem.hit == hit {
                                        Text(fetchProblem.message).koanText(.fine, .bad)
                                    }
                                }
                                Spacer()
                                if picking == hit {
                                    ProgressView().controlSize(.small)
                                } else if fetched == hit {
                                    Image(systemName: "checkmark")
                                        .foregroundStyle(KoanTheme.style(.accent, system: .tint))
                                }
                            }
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .disabled(picking != nil)
                    }
                }
                if let why = hits.prefix(20).first(where: { $0.locked != nil })?.locked {
                    Text(why + ".").koanText(.fine, .muted)
                }
            } header: {
                Text("Find it on squig.link")
            } footer: {
                Text("Measurements reviewers publish on their squig.link sites. Pick one measured on the rig your target assumes, where the site says, and to match a site's own EQ presets, its measurement; the site is credited on the correction.")
                    .koanText(.fine, .muted)
            }
            Section {
                Button(file.map { "Chosen: \($0)" } ?? "Choose a File…") { choosing = true }
            } footer: {
                Text("A .csv or .txt file of frequency and level. For a speaker measured by Audio Science Review, choose both its SPL Horizontal and SPL Vertical files.")
                    .koanText(.fine, .muted)
            }
            Section {
                TextEditor(text: $text)
                    .font(.system(.caption, design: .monospaced))
                    .frame(minHeight: 140)
            } header: {
                Text("Or paste it here")
            }
        }
    }

    private var ear: some View {
        Section {
            Picker("Your headphones are", selection: $inEar) {
                Text("In-ear").tag(true)
                Text("Over-ear").tag(false)
            }
            .pickerStyle(.inline)
            .labelsHidden()
        } footer: {
            Text(inEar
                 ? "In-ear: earphones and IEMs that sit in your ear canal."
                 : "Over-ear: headphones with cups that sit around or on your ears.")
        }
    }

    private var targetStep: some View {
        Group {
            if speaker, let reading {
                Section("Read as") {
                    Text(reading.note).koanText(.meta, .muted)
                }
            }
            targetPicker
        }
    }

    private var targetPicker: some View {
        Section {
            Picker("Target", selection: $target) {
                ForEach(targets, id: \.id) { t in
                    VStack(alignment: .leading, spacing: 2) {
                        Text(t.label)
                        if !t.character.isEmpty {
                            Text(t.character)
                                .font(.role(.fine, system: .caption))
                                .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
                        }
                    }
                    .tag(Optional(t.id))
                }
            }
            .pickerStyle(.inline)
            .labelsHidden()
        } footer: {
            Text("Each target is a different idea of a good sound. You can change it later on the correction's page.")
                .koanText(.fine, .muted)
        }
    }

    private var review: some View {
        Group {
            if let preview {
                Section {
                    EqGraph(response: preview, startOn: .headphone)
                } footer: {
                    Text("Measured shows your measurement, the target, and what your \(speaker ? "speaker" : "headphones") will sound like with the correction. EQ shows the correction itself.")
                        .koanText(.fine, .muted)
                }
            }
            Section {
                TextField("Name", text: $name)
            } footer: {
                Text("Usually the \(speaker ? "speaker's" : "headphones'") name. It becomes a correction: add tunings, like more bass or a room tilt, on top of it.")
                    .koanText(.fine, .muted)
            }
        }
    }

    // MARK: - Doing

    /// A speaker's measurement: corrected to Flat, the one speaker target,
    /// with no headphone target offered.
    private var speaker: Bool { reading?.speaker == true }

    private var canGoOn: Bool {
        switch step {
        case .learn, .ear: true
        case .file: !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        case .target: target != nil
        case .review: false
        }
    }

    private func next() async {
        problem = nil
        switch step {
        case .ear:
            targets = await dsp.targetsFor(inEar: inEar)
            // Harman, as AutoEQ's corrections are, unless one was picked:
            // what most people want. Neutral is listed first for the rest.
            if !targets.contains(where: { $0.id == target }) {
                let harman = inEar ? "harman-in-ear-2019" : "harman-over-ear-2018"
                target = targets.first { $0.id == harman }?.id ?? targets.first?.id
            }
        case .target:
            guard let target else { return }
            do {
                preview = try await dsp.previewMeasurement(text, target: target)
            } catch {
                problem = SettingsModel.describe(error)
                return
            }
        case .file:
            // Checked now, so a file that is not a measurement is said so
            // before the questions after it.
            do {
                reading = try await dsp.describeMeasurement(text)
            } catch {
                problem = SettingsModel.describe(error)
                return
            }
            if speaker {
                targets = await dsp.targetsFor(.speaker)
                target = targets.first { $0.id == "flat" }?.id ?? targets.first?.id
                step = .target
                return
            }
        default:
            break
        }
        step = Step(rawValue: step.rawValue + 1) ?? .review
    }

    /// Search squig.link's sites once typing pauses. A search overtaken by
    /// newer typing leaves what it found unshown.
    private func search() async {
        try? await Task.sleep(for: .milliseconds(300))
        guard !Task.isCancelled else { return }
        let query = squigQuery.trimmingCharacters(in: .whitespaces)
        guard !query.isEmpty else {
            hits = []
            searchNote = nil
            searching = false
            return
        }
        searching = true
        do {
            let found = try await dsp.squigSearch(query)
            guard !Task.isCancelled else { return }
            hits = found
            searchNote = found.isEmpty ? "Nothing on squig.link matches “\(query)”." : nil
        } catch {
            guard !Task.isCancelled else { return }
            hits = []
            searchNote = SettingsModel.describe(error)
        }
        searching = false
    }

    /// A result's site, and its rig where the site says.
    private func details(_ hit: SquigHit) -> String {
        [hit.siteLabel, hit.rig.map { "\($0) rig" }].compactMap { $0 }.joined(separator: " · ")
    }

    /// Fetch `hit`'s measurement as the file: its name, its site credited,
    /// and in-ear or over-ear where the site keeps one kind. A failure is
    /// said under the result, where the person is looking.
    private func pick(_ hit: SquigHit) {
        picking = hit
        fetchProblem = nil
        Task {
            defer { picking = nil }
            do {
                text = try await dsp.squigFetch(hit)
                fetched = hit
                file = "\(hit.name), \(hit.siteLabel)"
                source = hit.source
                problem = nil
                if name.isEmpty { name = "\(hit.brand) \(hit.model)" }
                if let inEar = hit.inEar { self.inEar = inEar }
            } catch {
                fetchProblem = (hit, SettingsModel.describe(error))
            }
        }
    }

    /// Read the chosen files as one text: a speaker's two planes are read
    /// together. On the Mac, choosing one plane of an Audio Science Review
    /// export brings its sibling along where it can be read.
    private func read(_ chosen: [URL]) {
        var texts: [String] = []
        var names: [String] = []
        for url in chosen {
            let held = url.startAccessingSecurityScopedResource()
            defer { if held { url.stopAccessingSecurityScopedResource() } }
            guard let contents = try? String(contentsOf: url, encoding: .utf8) else {
                problem = "\(url.lastPathComponent) could not be read as text."
                return
            }
            texts.append(contents)
            names.append(url.lastPathComponent)
        }
        #if os(macOS)
        // Best effort: the picker grants the file chosen, and a protected
        // folder such as Downloads may refuse its sibling. Without it the
        // measurement reads as on-axis, and the note asks for the other plane.
        if chosen.count == 1, let sibling = Self.otherPlane(of: chosen[0]),
           let contents = try? String(contentsOf: sibling, encoding: .utf8) {
            texts.append(contents)
            names.append(sibling.lastPathComponent)
        }
        #endif
        problem = nil
        text = texts.joined(separator: "\n")
        file = names.joined(separator: ", ")
        source = nil
        if name.isEmpty {
            // Audio Science Review names the files by plane, and their
            // folder by the speaker.
            let first = chosen[0]
            name = Self.otherPlane(of: first) == nil
                ? first.deletingPathExtension().lastPathComponent
                : first.deletingLastPathComponent().lastPathComponent
        }
    }

    /// The other plane's file beside one of an Audio Science Review export.
    private static func otherPlane(of url: URL) -> URL? {
        let other: String? = switch url.lastPathComponent {
        case "SPL Horizontal.txt": "SPL Vertical.txt"
        case "SPL Vertical.txt": "SPL Horizontal.txt"
        default: nil
        }
        return other.map { url.deletingLastPathComponent().appendingPathComponent($0) }
    }

    private func save() {
        guard let target else { return }
        saving = true
        Task {
            do {
                let ear: DspEarKind = speaker ? .speaker : inEar ? .inEar : .overEar
                let saved = try await dsp.saveMeasured(name: name, text: text, ear: ear, target: target, source: source)
                self.saved(saved)
                dismiss()
            } catch {
                problem = SettingsModel.describe(error)
            }
            saving = false
        }
    }
}
#endif
