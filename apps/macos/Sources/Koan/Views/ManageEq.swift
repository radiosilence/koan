import KoanFFI
import SwiftUI

/// Every correction, EQ and preset, where each is used, and what can be done
/// to it: open, rename, copy, revert, keep on every device, delete. Groups
/// are headings, their members under them. Reached from the EQ page, which
/// shows one device; this shows them all.
struct ManageEq: View {
    @Environment(AppState.self) private var app
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
            #if !os(tvOS)
            Section {
                Button("Import a File…") { importing = true }
                    .koanButton(.bordered)
                Button("Find in AutoEQ…") { finding = AutoEqFind(query: "") }
                    .koanButton(.bordered)
                Button("Find a Measurement…") { measuring = true }
                    .koanButton(.bordered)
                if let summary = dsp.importSummary {
                    Text(summary).koanText(.fine, .muted)
                }
                if let error = dsp.lastError {
                    Text(error).koanText(.fine, .bad)
                }
            } header: {
                KoanSectionHeader("Add")
            } footer: {
                Text("AutoEQ and Equalizer APO text, impulse WAVs, Roon zips, Convolver .cfg and CamillaDSP configs, or a headphone found in AutoEQ by name. Importing under a name already used adds to that EQ; several files at once become a group.")
                    .koanText(.fine, .muted)
            }
            #endif
        }
        .navigationTitle(KoanTheme.label("Manage EQ"))
        .task(id: dsp.stamp) { dsp.reload() }
        #if !os(tvOS)
        .filePicker(
            isPresented: $importing,
            allowedContentTypes: [.item, .folder],
            allowsMultipleSelection: true
        ) { result in
            if case let .success(urls) = result, !urls.isEmpty {
                dsp.importFiles(urls)
            }
        }
        .formTray(item: $finding) { find in
            AutoEqSearch(dsp: dsp, query: find.query)
        }
        .formTray(isPresented: $measuring) {
            MeasurementFlow(dsp: dsp)
        }
        .formTray(item: Binding(
            get: { dsp.askRole },
            // Swiped away, as Decide Later.
            set: { if $0 == nil, let ask = dsp.askRole { dsp.answer(ask, nil) } }
        )) { ask in
            RoleQuestion(dsp: dsp, ask: ask)
        }
        .alert("Rename", isPresented: Binding(get: { renaming != nil }, set: { if !$0 { renaming = nil } })) {
            TextField("Name", text: $newName)
            Button("Rename") {
                if let old = renaming?.name { Task { _ = await dsp.rename(old, to: newName) } }
            }
            Button("Cancel", role: .cancel) {}
        }
        .alert("Save as New", isPresented: Binding(get: { copying != nil }, set: { if !$0 { copying = nil } })) {
            TextField("Name", text: $newName)
            Button("Save") {
                if let name = copying?.name {
                    let new = newName.trimmingCharacters(in: .whitespaces)
                    dsp.duplicate(name, as: new.isEmpty ? nil : new)
                }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("A copy as it is now, to change apart from the original.")
        }
        .confirmationDialog(
            "Delete \(deleting?.name ?? "")?",
            isPresented: Binding(get: { deleting != nil }, set: { if !$0 { deleting = nil } }),
            titleVisibility: .visible
        ) {
            Button("Delete", role: .destructive) {
                if let name = deleting?.name { dsp.remove(name) }
            }
        } message: {
            Text(deleteMessage)
        }
        #endif
        #if os(iOS)
        .navigationDestination(item: $showing) { shown in
            DspProfilePage(dsp: dsp, name: shown.name, device: device)
                .koanPushedPage()
        }
        #elseif os(macOS)
        .sheet(item: $showing) { shown in
            NavigationStack {
                DspProfilePage(dsp: dsp, name: shown.name, device: device)
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
                #if !os(tvOS)
                if let device, group.role == .tuning {
                    Button("Add All to Tuning") { addToTuning(group.members, device) }
                        .koanButton(.link)
                }
                Menu("Options") { actions(group, o) }
                    .koanControl()
                    .fixedSize()
                #endif
            }
        }
    }

    private func row(_ p: DspProfileSummary, _ o: DspOverview) -> some View {
        HStack {
            #if os(tvOS)
            // Read here, changed on a phone or computer.
            summary(p)
            #else
            Button { showing = ShownProfile(name: p.name) } label: {
                summary(p)
            }
            .buttonStyle(.plain)
            Menu("Options") { actions(p, o) }
                .koanControl()
                .fixedSize()
            #endif
        }
        #if !os(tvOS)
        .contextMenu { actions(p, o) }
        #endif
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

    #if !os(tvOS)
    @ViewBuilder private func actions(_ p: DspProfileSummary, _ o: DspOverview) -> some View {
        let dsp = app.dsp
        Button("Open") { showing = ShownProfile(name: p.name) }
        if let device {
            if p.preset {
                Button("Use on \(dsp.label(device))") { dsp.applyPreset(p.name, to: device) }
            } else if p.role == .tuning, p.rates.isEmpty, !chain.contains(where: { $0.name == p.name }) {
                Button("Add to Tuning") { addToTuning([p.name], device) }
            } else if p.role != .tuning, active != p.name {
                Button("Use as Correction") { dsp.assign(p.name, to: device) }
            }
        }
        Divider()
        Button("Rename…") {
            newName = p.name
            renaming = ShownProfile(name: p.name)
        }
        Button("Save as New…") {
            newName = "\(p.name) copy"
            copying = ShownProfile(name: p.name)
        }
        if p.edited {
            Button("Revert to Imported") { dsp.revert(p.name) }
        }
        // A button rather than a toggle: a menu draws a toggle in the
        // environment's style, and the theme's box comes apart there.
        if let why = p.scopeLocked {
            Button {} label: {
                Text(p.everywhere ? "Kept on Every Device" : "Kept on This Device")
                Text(why)
            }
            .disabled(true)
        } else {
            Button(p.everywhere ? "Keep on This Device Only" : "Keep on Every Device") {
                dsp.setScope(p.name, everywhere: !p.everywhere)
            }
        }
        Divider()
        Button("Delete…", role: .destructive) { deleting = ShownProfile(name: p.name) }
    }
    #endif

    private func addToTuning(_ names: [String], _ device: String) {
        let added = names.filter { n in !chain.contains { $0.name == n } }
        app.dsp.setTunings(chain + added.map { DspTuningEntry(name: $0, on: true) }, for: device)
    }
}
