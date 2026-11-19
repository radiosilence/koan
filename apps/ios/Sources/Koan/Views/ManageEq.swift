import KoanFFI
import SwiftUI

/// Every correction, EQ and preset, where each is used, and what can be done
/// to it: open, rename, copy, revert, keep on every device, delete. Groups
/// are headings, their members under them. Reached from the EQ page, which
/// shows one device; this shows them all.
struct ManageEq: View {
    @Environment(AppState.self) private var app
    @Environment(EngineMirror.self) private var mirror
    /// The device the EQ page shows, which Add to Tuning adds to, with its
    /// correction and tuning.
    let device: String?
    let active: String?
    let chain: [DspTuningEntry]

    @State private var importing = false
    @State private var finding: AutoEqFind?
    @State private var measuring = false
    @State private var showing: ShownProfile?
    @State private var renaming: ShownProfile?
    @State private var copying: ShownProfile?
    @State private var deleting: ShownProfile?
    @State private var newName = ""

    var body: some View {
        let dsp = app.dsp
        KoanForm {
            if let o = dsp.overview {
                let grouped = Set(o.profiles.flatMap(\.members))
                let loose = o.profiles.filter { !$0.preset && $0.members.isEmpty && !grouped.contains($0.name) }
                list("Corrections", loose.filter { $0.role != .tuning }, o)
                list("EQs", loose.filter { $0.role == .tuning }, o)
                ForEach(o.profiles.filter { !$0.members.isEmpty }, id: \.name) { group in
                    groupSection(group, o)
                }
                list("Presets", o.profiles.filter(\.preset), o)
            }
        }
        .navigationTitle(KoanTheme.label("Manage EQ"))
        .task(id: dsp.stamp) { dsp.reload() }
    }

    /// Where it goes from: the devices that play it, and the presets and
    /// groups that hold it, which go on without it.
    private var deleteMessage: String {
        guard let name = deleting?.name,
              let p = app.dsp.overview?.profiles.first(where: { $0.name == name })
        else { return "" }
        var lines: [String] = []
        if !p.usedOn.isEmpty {
            lines.append("Used on \(usedOn(p)), which will play without it.")
        }
        if !p.heldBy.isEmpty {
            lines.append("It is taken out of \(ListFormatter.localizedString(byJoining: p.heldBy)).")
        }
        return lines.isEmpty ? "It is not used anywhere." : lines.joined(separator: " ")
    }

    @ViewBuilder private func list(_ title: String, _ profiles: [DspProfileSummary], _ o: DspOverview) -> some View {
        if !profiles.isEmpty {
            Section {
                ForEach(profiles, id: \.name) { row($0, o) }
            } header: {
                KoanSectionHeader(title)
            }
        }
    }

    /// A group: a heading for the EQs or corrections in it, one of which
    /// plays where the group is chosen. EQs can go into the tuning one at a
    /// time or all at once.
    @ViewBuilder private func groupSection(_ group: DspProfileSummary, _ o: DspOverview) -> some View {
        let members = group.members.compactMap { name in o.profiles.first { $0.name == name } }
        Section {
            ForEach(members, id: \.name) { row($0, o) }
        } header: {
            HStack {
                KoanSectionHeader(group.name)
                Spacer()
            }
        }
    }

    private func row(_ p: DspProfileSummary, _ o: DspOverview) -> some View {
        HStack {
            // Read here, changed on a phone or computer.
            summary(p)
        }
    }

    private func summary(_ p: DspProfileSummary) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Text(p.name)
                if p.edited {
                    Text("Edited").koanText(.fine, .muted)
                }
            }
            if let problem = p.problem {
                Text(problem).koanText(.fine, .bad)
            } else {
                let what = DspModel.describe(p)
                Text([what, p.usedOn.isEmpty ? "Not used" : "Used on \(usedOn(p))"].filter { !$0.isEmpty }.joined(separator: " · "))
                    .koanText(.fine, .muted)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .contentShape(Rectangle())
    }

    private func usedOn(_ p: DspProfileSummary) -> String {
        ListFormatter.localizedString(byJoining: p.usedOn.map(app.dsp.label))
    }


    private func addToTuning(_ names: [String], _ device: String) {
        let added = names.filter { n in !chain.contains { $0.name == n } }
        app.dsp.setTunings(chain + added.map { DspTuningEntry(name: $0, on: true) }, for: device)
    }
}

