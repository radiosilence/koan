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
    @State private var editingName = ""
    @State private var confirmingDelete = false

    var body: some View {
        Form {
            Section {
                TextField("Name", text: $editingName)
                    .onSubmit(rename)
            }

            if let d = detail {
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
                    ForEach(d.devices, id: \.self) { Text($0) }
                    if let device = dsp.overview?.device {
                        if d.devices.contains(device) {
                            Button("Stop using for \(device)") { dsp.use(nil) }
                        } else {
                            Button("Use for \(device)") { dsp.use(d.name) }
                        }
                    }
                }

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

                if !d.bands.isEmpty {
                    Section("EQ") {
                        ForEach(Array(d.bands.enumerated()), id: \.offset) { _, band in
                            BandRow(band: band)
                        }
                    }
                }

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

                Section {
                    Button("Delete Profile", role: .destructive) { confirmingDelete = true }
                }
            } else {
                ProgressView()
            }
        }
        .formStyle(.grouped)
        .navigationTitle(name)
        .task(id: dsp.version) { await load() }
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

    private func load() async {
        detail = await dsp.detail(name)
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

private struct BandRow: View {
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
        case "gain": "Gain"
        default: band.kind
        }
        let channels = band.channels.map { c in
            switch c {
            case 0: "L"
            case 1: "R"
            default: "Ch \(c + 1)"
            }
        }
        return channels.isEmpty ? name : "\(name) · \(channels.joined(separator: " "))"
    }

    private var values: String {
        let gain = String(format: "%+.1f dB", band.gainDb)
        if band.kind == "gain" { return gain }
        let freq = band.freq >= 1000
            ? String(format: "%.1f kHz", band.freq / 1000)
            : String(format: "%.0f Hz", band.freq)
        let q = String(format: "Q %.2f", band.q)
        return ["low_pass", "high_pass", "notch", "band_pass", "all_pass"].contains(band.kind)
            ? "\(freq)  \(q)"
            : "\(freq)  \(gain)  \(q)"
    }
}
