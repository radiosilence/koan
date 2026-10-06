import KoanFFI
import SwiftUI

/// A profile's filters as a table, its parametric bands editable in place:
/// number, type, frequency, gain and Q. An edit plays at once. Delays, mixes
/// and graphic curves are shown as they are.
struct BandTable: View {
    let dsp: DspModel
    let profile: String
    let bands: [DspBand]

    /// The band types that can be chosen, by the name the config uses.
    static let kinds: [(id: String, name: String)] = [
        ("peaking", "Peak"),
        ("low_shelf", "Low shelf"),
        ("high_shelf", "High shelf"),
        ("low_pass", "Low pass"),
        ("high_pass", "High pass"),
        ("notch", "Notch"),
        ("band_pass", "Band pass"),
        ("all_pass", "All pass"),
    ]

    static func editable(_ kind: String) -> Bool {
        kinds.contains { $0.id == kind }
    }

    /// The bands the graph draws a handle for: the ones with a gain to drag,
    /// on the left channel the graph draws.
    static func handles(_ bands: [DspBand]) -> [EqGraph.Handle] {
        bands.enumerated().compactMap { i, b in
            ["peaking", "low_shelf", "high_shelf"].contains(b.kind) && (b.channels.isEmpty || b.channels.contains(0))
                ? EqGraph.Handle(index: i, hz: b.freq, db: b.gainDb)
                : nil
        }
    }

    var body: some View {
        Section {
            if !bands.isEmpty {
                HStack(spacing: 8) {
                    Text("#").frame(width: 22, alignment: .leading)
                    Text("Type").frame(maxWidth: .infinity, alignment: .leading)
                    Text("Hz").frame(width: 72, alignment: .trailing)
                    Text("dB").frame(width: 56, alignment: .trailing)
                    Text("Q").frame(width: 50, alignment: .trailing)
                }
                .koanText(.fine, .muted)
            }
            ForEach(Array(bands.enumerated()), id: \.offset) { index, band in
                Group {
                    if Self.editable(band.kind) {
                        BandEditor(dsp: dsp, profile: profile, index: index, band: band)
                    } else {
                        BandRow(band: band)
                    }
                }
                #if !os(tvOS)
                .contextMenu {
                    Button(KoanTheme.label("Remove"), role: .destructive) { dsp.removeFilter(profile, index) }
                }
                #endif
            }
            #if os(iOS)
            .onDelete { offsets in
                // One at a time, from the end, so the indices hold.
                for index in offsets.sorted(by: >) { dsp.removeFilter(profile, index) }
            }
            #endif
            #if !os(tvOS)
            Button(KoanTheme.label("Add a Band")) { dsp.addBand(profile) }
            #endif
        } header: {
            KoanSectionHeader("Filters")
        } footer: {
            Text("Edits play at once. Frequency, gain and Q are held to 10 Hz–22 kHz, ±30 dB and 0.1–20.")
                .koanText(.fine, .muted)
        }
    }
}

/// One band's row, each field committed when it is left or returned.
private struct BandEditor: View {
    let dsp: DspModel
    let profile: String
    let index: Int
    let band: DspBand

    @State private var kind = ""
    @State private var freq = 0.0
    @State private var gain = 0.0
    @State private var q = 0.0
    @FocusState private var focused: Field?

    enum Field { case freq, gain, q }

    var body: some View {
        HStack(spacing: 8) {
            Text("\(index + 1)")
                .koanText(.body, .muted)
                .monospacedDigit()
                .frame(width: 22, alignment: .leading)
            Picker("Type", selection: Binding(get: { kind }, set: { kind = $0; commit() })) {
                ForEach(BandTable.kinds, id: \.id) { Text($0.name).tag($0.id) }
            }.koanControl()
            .labelsHidden()
            .frame(maxWidth: .infinity, alignment: .leading)
            field($freq, .freq, width: 72, digits: 0)
            field($gain, .gain, width: 56, digits: 1)
            field($q, .q, width: 50, digits: 2)
        }
        .onChange(of: focused) { was, _ in if was != nil { commit() } }
        #if os(iOS)
        // The decimal pad has no return key.
        .toolbar {
            if focused != nil {
                ToolbarItemGroup(placement: .keyboard) {
                    Spacer()
                    Button(KoanTheme.label("Done")) { focused = nil }
                }
            }
        }
        #endif
        .onAppear(perform: read)
        .onChange(of: band.freq) { _, _ in read() }
        .onChange(of: band.gainDb) { _, _ in read() }
        .onChange(of: band.q) { _, _ in read() }
        .onChange(of: band.kind) { _, _ in read() }
    }

    private func field(_ value: Binding<Double>, _ name: Field, width: CGFloat, digits: Int) -> some View {
        TextField("", value: value, format: .number.precision(.fractionLength(0 ... digits)))
            .focused($focused, equals: name)
            .multilineTextAlignment(.trailing)
            .monospacedDigit()
            .frame(width: width)
            .onSubmit(commit)
            #if os(iOS)
            .keyboardType(.decimalPad)
            #endif
    }

    private func read() {
        (kind, freq, gain, q) = (band.kind, band.freq, band.gainDb, band.q)
    }

    private func commit() {
        guard kind != band.kind || freq != band.freq || gain != band.gainDb || q != band.q else { return }
        dsp.setBand(profile, index, kind: kind, freq: freq, gain: gain, q: q)
    }
}
