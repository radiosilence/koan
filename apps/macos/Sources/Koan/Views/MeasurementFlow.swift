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
    @State private var saving = false
    /// Searching squig.link's sites, from the name the flow was opened with.
    @State private var squigQuery: String
    @State private var hits: [SquigHit] = []
    @State private var searching = false
    /// Why a search shows nothing: no match, or no site reached.
    @State private var searchNote: String?
    /// The result being fetched.
    @State private var picking: SquigHit?
    /// Where the measurement came from, credited on the profile.
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
            case .learn: "Use a measurement"
            case .file: "Your measurement"
            case .ear: "In-ear or over-ear"
            case .target: "Choose a target"
            case .review: "Check and save"
            }
        }
    }

    var body: some View {
        NavigationStack {
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
                }
            }
            .navigationTitle(KoanTheme.label(step.title))
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    if step == .review {
                        Button("Save") { save() }
                            .disabled(name.trimmingCharacters(in: .whitespaces).isEmpty || saving)
                    } else {
                        Button("Next") { Task { await next() } }
                            .disabled(!canGoOn)
                    }
                }
                if step != .learn {
                    ToolbarItem(placement: .navigation) {
                        Button("Back") {
                            problem = nil
                            step = Step(rawValue: step.rawValue - 1) ?? .learn
                        }
                    }
                }
            }
            .fileImporter(
                isPresented: $choosing,
                allowedContentTypes: [.commaSeparatedText, .plainText, .text, .data]
            ) { result in
                if case let .success(url) = result { read(url) }
            }
        }
        #if os(macOS)
        .frame(minWidth: 520, minHeight: 560)
        #endif
    }

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
                TextField("Headphones", text: $squigQuery)
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
                    Button { pick(hit) } label: {
                        HStack {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(hit.name)
                                Text([hit.siteLabel, hit.rig.map { "\($0) rig" }].compactMap { $0 }.joined(separator: " · "))
                                    .koanText(.fine, .muted)
                            }
                            Spacer()
                            if picking == hit {
                                ProgressView().controlSize(.small)
                            }
                        }
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .disabled(picking != nil)
                }
            } header: {
                Text("Find it on squig.link")
            } footer: {
                Text("Measurements reviewers publish on their squig.link sites. Pick one measured on the rig your target assumes, where the site says; the site is credited on the profile.")
                    .koanText(.fine, .muted)
            }
            Section {
                Button(file.map { "Chosen: \($0)" } ?? "Choose a File…") { choosing = true }
            } footer: {
                Text("A .csv or .txt file of frequency and level.")
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
                    Text("Measured shows your measurement, the target, and what your headphones will sound like with the correction. EQ shows the correction itself.")
                        .koanText(.fine, .muted)
                }
            }
            Section {
                TextField("Name", text: $name)
            } footer: {
                Text("Usually the headphones' name. It becomes a correction: add tunings, like more bass, on top of it.")
                    .koanText(.fine, .muted)
            }
        }
    }

    // MARK: - Doing

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
                _ = try await dsp.previewMeasurement(text, target: inEar ? "harman-in-ear-2019" : "harman-over-ear-2018")
            } catch {
                problem = SettingsModel.describe(error)
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

    /// Fetch `hit`'s measurement as the file: its name, its site credited,
    /// and in-ear or over-ear where the site keeps one kind.
    private func pick(_ hit: SquigHit) {
        picking = hit
        Task {
            defer { picking = nil }
            do {
                text = try await dsp.squigFetch(hit)
                file = "\(hit.name), \(hit.siteLabel)"
                source = hit.source
                problem = nil
                if name.isEmpty { name = "\(hit.brand) \(hit.model)" }
                if let inEar = hit.inEar { self.inEar = inEar }
            } catch {
                problem = SettingsModel.describe(error)
            }
        }
    }

    private func read(_ url: URL) {
        let held = url.startAccessingSecurityScopedResource()
        defer { if held { url.stopAccessingSecurityScopedResource() } }
        guard let contents = try? String(contentsOf: url, encoding: .utf8) else {
            problem = "That file could not be read as text."
            return
        }
        text = contents
        file = url.lastPathComponent
        source = nil
        if name.isEmpty {
            name = url.deletingPathExtension().lastPathComponent
        }
    }

    private func save() {
        guard let target else { return }
        saving = true
        Task {
            do {
                let saved = try await dsp.saveMeasured(name: name, text: text, inEar: inEar, target: target, source: source)
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
