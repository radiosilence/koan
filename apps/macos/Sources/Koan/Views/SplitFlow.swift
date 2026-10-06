#if !os(tvOS)
import KoanFFI
import SwiftUI
import UniformTypeIdentifiers

/// Taking a baked EQ apart: with a measurement of the headphones and the
/// target that counts as neutral, the correction is target minus
/// measurement, and the tuning is what the EQ does beyond it. Both are drawn
/// against the EQ before anything is saved.
struct SplitFlow: View {
    let dsp: DspModel
    let name: String
    @Environment(\.dismiss) private var dismiss

    @State private var text = ""
    @State private var file: String?
    @State private var choosing = false
    @State private var inEar = true
    @State private var targets: [DspTargetOption] = []
    @State private var target: String?
    @State private var preview: DspResponse?
    @State private var problem: String?
    @State private var saving = false

    var body: some View {
        NavigationStack {
            KoanForm {
                Section {
                    Text("\(name) is a correction for a device with a tuning baked in. With a measurement of the device, kōan takes it apart: a **correction** that makes them neutral, and the **tuning**, your taste, which then works on any headphones.")
                }
                Section {
                    Button(file ?? "Choose a Measurement File…") { choosing = true }
                    Picker("Headphones", selection: $inEar) {
                        Text("In-ear").tag(true)
                        Text("Over-ear").tag(false)
                    }
                } header: {
                    Text("Measurement")
                } footer: {
                    Text("A frequency response of the device, from squig.link or REW.")
                        .koanText(.fine, .muted)
                }
                if !targets.isEmpty {
                    Section {
                        Picker("Neutral is", selection: $target) {
                            ForEach(targets, id: \.id) { t in
                                TargetRow(target: t).tag(Optional(t.id))
                            }
                        }
                        #if os(iOS)
                        .pickerStyle(.navigationLink)
                        #endif
                    } footer: {
                        Text("The target \(name) was made for. Harman is the usual one; the tuning is what it does beyond it.")
                            .koanText(.fine, .muted)
                    }
                }
                if let preview {
                    Section {
                        EqGraph(response: preview)
                    } footer: {
                        Text("Correction and tuning together play as \(name) does, but for the treble a correction holds back, where measurements disagree.")
                            .koanText(.fine, .muted)
                    }
                }
                if let problem {
                    Section {
                        Label(problem, systemImage: "exclamationmark.triangle.fill")
                            .koanText(.meta, .bad)
                    }
                }
                Section {
                    Text("Split makes “\(name) correction” and “\(name) tuning”. The outputs that play \(name) will play the correction with the tuning on top, and \(name) itself is kept.")
                        .koanText(.meta, .muted)
                }
            }
            .navigationTitle("Split \(name)")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Split") { split() }
                        .disabled(preview == nil || saving)
                }
            }
            .fileImporter(
                isPresented: $choosing,
                allowedContentTypes: [.commaSeparatedText, .plainText, .text, .data]
            ) { result in
                if case let .success(url) = result { read(url) }
            }
            .task(id: inEar) {
                targets = await dsp.targetsFor(inEar: inEar)
                if !targets.contains(where: { $0.id == target }) {
                    let harman = inEar ? "harman-in-ear-2019" : "harman-over-ear-2018"
                    target = targets.first { $0.id == harman }?.id ?? targets.first?.id
                }
            }
            .task(id: "\(file ?? "")\u{0}\(target ?? "")") { await refresh() }
        }
        #if os(macOS)
        .frame(minWidth: 520, minHeight: 600)
        #endif
    }

    private func refresh() async {
        guard !text.isEmpty, let target else { return }
        do {
            preview = try await dsp.previewSplit(name, text: text, target: target)
            problem = nil
        } catch {
            preview = nil
            problem = SettingsModel.describe(error)
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
    }

    private func split() {
        guard let target else { return }
        saving = true
        Task {
            do {
                _ = try await dsp.splitBaked(name, text: text, inEar: inEar, target: target)
                dismiss()
            } catch {
                problem = SettingsModel.describe(error)
            }
            saving = false
        }
    }
}
#endif
