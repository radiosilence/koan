import KoanFFI
import SwiftUI

/// A profile's filters as a table, its parametric bands editable in place:
/// number, type, frequency, gain and Q. An edit plays at once. A graphic
/// curve opens a page of its points; delays and mixes are shown as they are.
struct BandTable: View {
    let dsp: DspModel
    let profile: String
    let bands: [DspBand]
    /// A correction, which plays as made: its bands shown, not edited.
    var readOnly = false

    /// The band types that can be chosen, by the name the config uses: the
    /// short form the row shows and the name the menu gives.
    static let kinds: [(id: String, short: String, name: String)] = [
        ("peaking", "PK", "Peak"),
        ("low_shelf", "LS", "Low shelf"),
        ("high_shelf", "HS", "High shelf"),
        ("low_pass", "LP", "Low pass"),
        ("high_pass", "HP", "High pass"),
        ("notch", "NO", "Notch"),
        ("band_pass", "BP", "Band pass"),
        ("all_pass", "AP", "All pass"),
    ]

    /// The table is read across rows of figures, so it is set small and tight.
    static let rowInsets = EdgeInsets(top: 4, leading: 16, bottom: 4, trailing: 16)

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
                .listRowInsets(Self.rowInsets)
            }
            ForEach(Array(bands.enumerated()), id: \.offset) { index, band in
                Group {
                    if Self.editable(band.kind), !readOnly {
                        BandEditor(dsp: dsp, profile: profile, index: index, band: band)
                    } else if band.kind == "graphic", !readOnly {
                        #if os(tvOS)
                        BandRow(band: band)
                        #else
                        NavigationLink {
                            CurvePage(dsp: dsp, profile: profile, index: index)
                        } label: {
                            BandRow(band: band)
                        }
                        #endif
                    } else {
                        BandRow(band: band)
                    }
                }
                .font(.role(.meta, system: .body))
                .listRowInsets(Self.rowInsets)
                #if !os(tvOS)
                .contextMenu {
                    if !readOnly {
                        Button("Remove", role: .destructive) { dsp.removeFilter(profile, index) }
                    }
                }
                #endif
            }
            #if os(iOS)
            .onDelete(perform: readOnly ? nil : { offsets in
                // One at a time, from the end, so the indices hold.
                for index in offsets.sorted(by: >) { dsp.removeFilter(profile, index) }
            })
            #endif
            #if !os(tvOS)
            if !readOnly {
                Button("Add a Band") { dsp.addBand(profile) }
                    .koanButton(.bordered)
            }
            #endif
        } header: {
            KoanSectionHeader("Filters")
        } footer: {
            Text(readOnly
                 ? "A correction plays as made. To change the sound, add a tuning on top; to edit these, make it a tuning under What it's for."
                 : "Edits play at once. Frequency, gain and Q are held to 10 Hz–22 kHz, ±30 dB and 0.1–20.")
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
                .koanText(.meta, .muted)
                .monospacedDigit()
                .frame(width: 22, alignment: .leading)
            // The short form in the row, the names in the menu.
            Menu {
                Picker("Type", selection: Binding(get: { kind }, set: { kind = $0; commit() })) {
                    ForEach(BandTable.kinds, id: \.id) { Text($0.name).tag($0.id) }
                }
            } label: {
                Text(BandTable.kinds.first { $0.id == kind }?.short ?? kind)
            }
            // Not `koanControl`, whose `control` type would outweigh the row's.
            .tint(KoanTheme.style(.ink, system: .tint))
            .accessibilityLabel(BandTable.kinds.first { $0.id == kind }?.name ?? kind)
            .frame(maxWidth: .infinity, alignment: .leading)
            NumberField(value: $freq, focus: $focused, name: .freq, width: 72, digits: 0, commit: commit)
            NumberField(value: $gain, focus: $focused, name: .gain, width: 56, digits: 1, commit: commit)
            NumberField(value: $q, focus: $focused, name: .q, width: 50, digits: 2, commit: commit)
        }
        .onChange(of: focused) { was, _ in if was != nil { commit() } }
        .decimalPadDone($focused)
        .onAppear(perform: read)
        .onChange(of: band.freq) { _, _ in read() }
        .onChange(of: band.gainDb) { _, _ in read() }
        .onChange(of: band.q) { _, _ in read() }
        .onChange(of: band.kind) { _, _ in read() }
    }

    private func read() {
        (kind, freq, gain, q) = (band.kind, band.freq, band.gainDb, band.q)
    }

    private func commit() {
        guard kind != band.kind || freq != band.freq || gain != band.gainDb || q != band.q else { return }
        dsp.setBand(profile, index, kind: kind, freq: freq, gain: gain, q: q)
    }
}

/// A figure in a row of the table, committed when it is left or returned,
/// with a rule beneath it so it reads as a field to tap.
private struct NumberField<Field: Hashable>: View {
    @Binding var value: Double
    var focus: FocusState<Field?>.Binding
    let name: Field
    let width: CGFloat
    let digits: Int
    let commit: () -> Void

    var body: some View {
        TextField("", value: $value, format: .number.precision(.fractionLength(0 ... digits)))
            .focused(focus, equals: name)
            .multilineTextAlignment(.trailing)
            .monospacedDigit()
            .frame(width: width)
            .onSubmit(commit)
            #if os(iOS)
            .keyboardType(.decimalPad)
            #endif
            .overlay(alignment: .bottom) {
                Rectangle()
                    .fill(KoanTheme.style(.rule, system: Color.secondary.opacity(0.4))) // theme: raw — the system look's own
                    .frame(height: KoanTheme.hairline)
                    .offset(y: 2)
            }
    }
}

private extension View {
    /// The decimal pad has no return key: a Done above it while a field of
    /// the row is focused.
    func decimalPadDone<Field: Hashable>(_ focus: FocusState<Field?>.Binding) -> some View {
        #if os(iOS)
        toolbar {
            if focus.wrappedValue != nil {
                ToolbarItemGroup(placement: .keyboard) {
                    Spacer()
                    Button("Done") { focus.wrappedValue = nil }
                }
            }
        }
        #else
        self
        #endif
    }
}

#if !os(tvOS)
/// A graphic curve's points, each frequency and gain editable, held to the
/// ranges a band has. An imported EQ can be put back as its file had it.
struct CurvePage: View {
    let dsp: DspModel
    let profile: String
    let index: Int

    @State private var detail: DspProfileDetail?
    @State private var confirmingReset = false

    private var points: [DspPoint] {
        guard let d = detail, d.bands.indices.contains(index) else { return [] }
        return d.bands[index].curve
    }

    var body: some View {
        KoanForm {
            Section {
                HStack(spacing: 8) {
                    Text("#").frame(width: 32, alignment: .leading)
                    Spacer()
                    Text("Hz").frame(width: 80, alignment: .trailing)
                    Text("dB").frame(width: 64, alignment: .trailing)
                }
                .koanText(.fine, .muted)
                .listRowInsets(BandTable.rowInsets)
                ForEach(Array(points.enumerated()), id: \.offset) { i, point in
                    PointEditor(index: i, point: point) { edited in
                        var changed = points
                        changed[i] = edited
                        dsp.setCurve(profile, index, changed)
                    }
                    .font(.role(.meta, system: .body))
                    .listRowInsets(BandTable.rowInsets)
                }
            } header: {
                KoanSectionHeader("Points")
            } footer: {
                Text("Edits play at once. Frequency and gain are held to 10 Hz–22 kHz and ±30 dB; a point moved past another takes its place in order.")
                    .koanText(.fine, .muted)
            }
            if let d = detail, d.canRevert, d.edited {
                Section {
                    Button("Reset to File") { confirmingReset = true }
                        .koanButton(.bordered)
                } footer: {
                    Text("Puts this EQ back as \(d.source.first ?? "its file") had it, every filter and the headroom with it.")
                        .koanText(.fine, .muted)
                }
            }
        }
        .navigationTitle(KoanTheme.label("Graphic EQ"))
        .task(id: dsp.stamp) { detail = await dsp.detail(profile) }
        #if os(iOS)
        .koanBackButton()
        .koanHidesSystemTabBar()
        #endif
        .confirmationDialog("Reset \(profile) to its file?", isPresented: $confirmingReset, titleVisibility: .visible) {
            Button("Reset", role: .destructive) { dsp.revert(profile) }
        }
    }
}

/// One point of a curve, each figure committed when it is left or returned.
private struct PointEditor: View {
    let index: Int
    let point: DspPoint
    let commit: (DspPoint) -> Void

    @State private var hz = 0.0
    @State private var db = 0.0
    @FocusState private var focused: Field?

    enum Field { case hz, db }

    var body: some View {
        HStack(spacing: 8) {
            Text("\(index + 1)")
                .koanText(.meta, .muted)
                .monospacedDigit()
                .frame(width: 32, alignment: .leading)
            Spacer()
            NumberField(value: $hz, focus: $focused, name: .hz, width: 80, digits: 1, commit: save)
            NumberField(value: $db, focus: $focused, name: .db, width: 64, digits: 1, commit: save)
        }
        .onChange(of: focused) { was, _ in if was != nil { save() } }
        .decimalPadDone($focused)
        .onAppear(perform: read)
        .onChange(of: point.hz) { _, _ in read() }
        .onChange(of: point.db) { _, _ in read() }
    }

    private func read() {
        (hz, db) = (point.hz, point.db)
    }

    private func save() {
        guard hz != point.hz || db != point.db else { return }
        commit(DspPoint(hz: hz, db: db))
    }
}
#endif
