import KoanFFI
import SwiftUI
import UniformTypeIdentifiers

/// Everything needed to set koan up, without opening a terminal.
///
/// The configuration lives in `config.toml`, shared with the CLI and the TUI, so
/// this is a view onto that file rather than a second source of truth: fields
/// commit when you finish editing, and the window re-reads on focus so a change
/// made elsewhere is not silently overwritten.
struct SettingsView: View {
    @Environment(PlayerModel.self) private var player
    @Environment(LibraryModel.self) private var library
    @Environment(ActivityModel.self) private var activity

    @State private var model: SettingsModel?
    #if os(macOS)
    @Environment(\.controlActiveState) private var controlActive
    #else
    @Environment(\.scenePhase) private var scenePhase
    #endif

    #if !os(macOS)
    /// A settings section: a row that goes into the pane it names.
    @ViewBuilder private func pane<Content: View>(
        _ title: String,
        _ symbol: String,
        @ViewBuilder _ content: @escaping () -> Content
    ) -> some View {
        NavigationLink {
            content().navigationTitle(title)
        } label: {
            Label(title, systemImage: symbol)
        }
    }
    #endif

    var body: some View {
        Group {
            if let model {
                #if os(macOS)
                // Four panes side by side in a window sized to hold the largest
                // of them, which is what a settings window is on macOS.
                TabView {
                    LibrarySettings(model: model)
                        .tabItem { Label("Library", systemImage: "music.note.house") }
                    RemoteSettings(model: model)
                        .tabItem { Label("Server", systemImage: "server.rack") }
                    PlaybackSettings(model: model)
                        .tabItem { Label("Playback", systemImage: "hifispeaker") }
                    RadioSettings(model: model)
                        .tabItem { Label("Radio", systemImage: "dot.radiowaves.left.and.right") }
                }
                .safeAreaInset(edge: .bottom) { StatusLine(model: model) }
                #else
                // A phone has no room for a second row of tabs — and it already
                // has one at the bottom of the screen. Settings on iOS is a list
                // you go into, so these become pages rather than panes.
                List {
                    // No Library pane. It is folders to scan, a scan to run and
                    // an index to clear — all of it about music sitting on a
                    // disk koan can walk. Inside the iOS sandbox there is no
                    // such disk, and koan is a Subsonic client and nothing else.
                    pane("Server", "server.rack") { RemoteSettings(model: model) }
                    pane("Playback", "hifispeaker") { PlaybackSettings(model: model) }
                    pane("Radio", "dot.radiowaves.left.and.right") { RadioSettings(model: model) }
                }
                .navigationTitle("Settings")
                .safeAreaInset(edge: .bottom) { StatusLine(model: model) }
                #endif
            } else {
                ProgressView()
            }
        }
        // The size of a settings window. A phone gets whatever it has.
        #if os(macOS)
        .frame(width: 560, height: 460)
        #endif
        .task {
            if model == nil {
                let created = await SettingsModel(engine: library.engine, activity: activity)
                created.library = library
                model = created
            }
        }
        // The CLI and TUI write the same file; coming back to this window is
        // the moment to notice they did.
        #if os(macOS)
        .onChange(of: controlActive) { _, state in
            if state != .inactive { model?.reload() }
        }
        #else
        .onChange(of: scenePhase) { _, phase in
            if phase == .active { model?.reload() }
        }
        #endif
    }
}

/// The result of the last action, or the reason it failed. One line, always in
/// the same place — an action that reports nothing looks like it did nothing.
private struct StatusLine: View {
    let model: SettingsModel

    var body: some View {
        Group {
            if let error = model.lastError {
                Label(error, systemImage: "exclamationmark.triangle.fill")
                    .foregroundStyle(.orange)
            } else if let result = model.lastResult {
                Label(result, systemImage: "checkmark.circle")
                    .foregroundStyle(.secondary)
            } else {
                Text(" ")
            }
        }
        .font(.caption)
        .lineLimit(2)
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 18)
        .padding(.vertical, 8)
        .background(.bar)
    }
}

// MARK: - Library

private struct LibrarySettings: View {
    @Bindable var model: SettingsModel
    @Environment(ActivityModel.self) private var activity
    @State private var confirmingRebuild = false
    @State private var removing: LibraryFolder?
    @State private var choosingFolder = false

    var body: some View {
        Form {
            Section {
                if model.settings.libraryFolders.isEmpty {
                    Text("No folders yet — koan has nothing to scan.")
                        .font(.callout)
                        .foregroundStyle(.secondary)
                }
                ForEach(model.settings.libraryFolders, id: \.path) { folder in
                    HStack {
                        Text(folder.path)
                            .font(.callout.monospaced())
                            .lineLimit(1)
                            .truncationMode(.head)
                            .help(folder.path)
                        Spacer(minLength: 8)
                        Text(Format.count(Int64(folder.tracks), "track"))
                            .font(.caption.monospacedDigit())
                            .foregroundStyle(.tertiary)
                        Button {
                            removing = folder
                        } label: {
                            Image(systemName: "minus.circle")
                        }
                        .buttonStyle(.borderless)
                        .help("Stop scanning this folder")
                    }
                }
                Button("Add Folder…") { choosingFolder = true }
                    .disabled(activity.isLibraryBusy)
            } header: {
                Text("Folders")
            } footer: {
                Text("Removing a folder stops it being scanned. It does not delete anything.")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
            }

            Section {
                HStack {
                    Button("Scan") { model.scan() }
                    Button("Rescan Everything") { model.scan(force: true) }
                        .help("Re-read every file's tags, ignoring the scan cache")
                }
                .rowButtons()
                // One library task at a time: they all queue behind the same
                // database writer, so starting a second only makes both slower.
                .disabled(activity.isLibraryBusy)
            } header: {
                Text("Scan")
            } footer: {
                if activity.isLibraryBusy {
                    Text("Waiting for the running task to finish.")
                        .font(.caption)
                        .foregroundStyle(.tertiary)
                }
            }

            Section {
                Button("Clear Library Index…", role: .destructive) {
                    confirmingRebuild = true
                }
                .disabled(activity.isLibraryBusy)
            } header: {
                Text("Rebuild")
            } footer: {
                Text("""
                    Forgets every artist, album and track so the next scan builds \
                    them again from your files. Favourites survive — they are kept \
                    against file paths. Lyrics, play counts and audio analysis do \
                    not; they are tied to rows that will not exist.
                    """)
                    .font(.caption)
                    .foregroundStyle(.tertiary)
            }
        }
        .formStyle(.grouped)
        .confirmationDialog(
            "Clear the library index?",
            isPresented: $confirmingRebuild,
            titleVisibility: .visible
        ) {
            Button("Clear Index", role: .destructive) { model.rebuildIndex() }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Play counts, lyrics and audio analysis are lost. Favourites are kept. Your music files are not touched.")
        }
        .confirmationDialog(
            "Stop scanning \(removing?.path ?? "")?",
            isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } }),
            titleVisibility: .visible
        ) {
            Button("Remove and Forget Its Tracks", role: .destructive) {
                if let folder = removing { model.removeFolder(folder.path, forgetTracks: true) }
                removing = nil
            }
            Button("Remove, Keep Them in the Library") {
                if let folder = removing { model.removeFolder(folder.path, forgetTracks: false) }
                removing = nil
            }
            Button("Cancel", role: .cancel) { removing = nil }
        } message: {
            Text("Your files are not touched either way. Keeping them leaves records in the library that koan will not scan again.")
        }
        .fileImporter(
            isPresented: $choosingFolder,
            allowedContentTypes: [.folder],
            allowsMultipleSelection: true
        ) { result in
            guard case .success(let urls) = result else { return }
            model.addFolders(urls.map(\.path))
        }
    }
}

// MARK: - Server

private struct RemoteSettings: View {
    @Bindable var model: SettingsModel
    @Environment(ActivityModel.self) private var activity
    @State private var url = ""
    @State private var username = ""
    @State private var confirmingSignOut = false

    var body: some View {
        Form {
            if model.settings.remoteSignedIn {
                Section("Signed in") {
                    LabeledContent("Server", value: model.settings.remoteUrl)
                    LabeledContent("User", value: model.settings.remoteUsername)
                    LabeledContent(
                        "Tracks",
                        value: Format.count(Int64(model.settings.remoteTracks), "track")
                    )
                    HStack {
                        // Only the syncs wait on the database writer. Signing
                        // out is a config write, and greying it out while a
                        // sync runs strands you on a server you are trying to
                        // leave.
                        Group {
                            Button("Sync Now") { model.syncNow(full: false) }
                            Button("Full Sync") { model.syncNow(full: true) }
                                .help("Walk the whole library rather than only what changed")
                        }
                        .disabled(activity.isLibraryBusy)
                        Spacer()
                        Button("Sign Out", role: .destructive) { confirmingSignOut = true }
                    }
                    .rowButtons()
                }
            } else {
                Section {
                    // Label on the left, example inside the field. Passing the
                    // example as the title made the URL the label.
                    TextField("Server", text: $url, prompt: Text("https://music.example.com"))
                        .verbatimEntry(.url)
                    TextField("Username", text: $username, prompt: Text("your account"))
                        .verbatimEntry()
                    SecureField(
                        "Password",
                        text: Binding(get: { model.password }, set: { model.password = $0 }),
                        prompt: Text("hunter2")
                    )
                    .verbatimEntry()
                    Button("Sign In") { model.signIn(url: url, username: username) }
                        .disabled(url.isEmpty || username.isEmpty || model.password.isEmpty)
                } header: {
                    Text("Subsonic or Navidrome")
                } footer: {
                    Text("Checked against the server, then saved to config.local.toml, readable only by you.")
                        .font(.caption)
                        .foregroundStyle(.tertiary)
                }
            }

            Section {
                Toggle("Keep the library in sync", isOn: Binding(
                    get: { model.settings.autoSync },
                    set: { on in model.edit { $0.autoSync = on } }
                ))
                if model.settings.autoSync {
                    Picker("Every", selection: Binding(
                        get: { model.settings.autoSyncIntervalMins },
                        set: { v in model.edit { $0.autoSyncIntervalMins = v } }
                    )) {
                        Text("Startup only").tag(UInt64(0))
                        Text("15 minutes").tag(UInt64(15))
                        Text("Hour").tag(UInt64(60))
                        Text("6 hours").tag(UInt64(360))
                        Text("Day").tag(UInt64(1440))
                    }
                }
            } header: {
                Text("Automatic sync")
            } footer: {
                Text("Incremental — it asks the server what changed. A full sync stays a deliberate choice.")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
            }

            Section("Downloads") {
                Stepper(
                    "Parallel downloads: \(model.settings.downloadWorkers)",
                    value: Binding(
                        get: { Int(model.settings.downloadWorkers) },
                        set: { v in model.edit { $0.downloadWorkers = UInt32(v) } }
                    ),
                    in: 1...16
                )
                TextField("Cache limit, e.g. 50GB — blank for no limit", text: Binding(
                    get: { model.settings.cacheLimit },
                    set: { v in model.edit { $0.cacheLimit = v } }
                ))
                .verbatimEntry()
                LabeledContent("Using") {
                    HStack {
                        Text(Format.bytes(Int64(model.settings.cacheBytes)))
                        Button("Clear") { model.clearCache() }
                            .buttonStyle(.borderless)
                    }
                }
            }
        }
        .formStyle(.grouped)
        .onAppear {
            url = model.settings.remoteUrl
            username = model.settings.remoteUsername
        }
        .confirmationDialog(
            "Sign out of \(model.settings.remoteUrl)?",
            isPresented: $confirmingSignOut,
            titleVisibility: .visible
        ) {
            Button("Sign Out and Forget Its Tracks", role: .destructive) {
                model.signOut(forgetTracks: true)
            }
            Button("Sign Out, Keep Them in the Library") {
                model.signOut(forgetTracks: false)
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Tracks you also have as local files are kept either way. Keeping the rest leaves records in the library that cannot be played until you sign in again.")
        }
    }
}

// MARK: - Playback

private struct PlaybackSettings: View {
    @Bindable var model: SettingsModel
    @Environment(PlayerModel.self) private var player

    var body: some View {
        Form {
            Section {
                Picker("Output", selection: Binding(
                    get: { player.currentDevice ?? "" },
                    set: { player.setDevice($0.isEmpty ? nil : $0) }
                )) {
                    Text("System Default").tag("")
                    ForEach(player.devices, id: \.name) { device in
                        Text(device.name).tag(device.name)
                    }
                }
            } header: {
                Text("Device")
            } footer: {
                Text("koan switches the device sample rate to match the source. No resampling.")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
            }

            Section {
                Picker("ReplayGain", selection: Binding(
                    get: { model.settings.replaygain },
                    set: { v in model.edit { $0.replaygain = v } }
                )) {
                    Text("Off").tag("off")
                    Text("Per track").tag("track")
                    Text("Per album").tag("album")
                }
                if model.settings.replaygain != "off" {
                    Stepper(
                        "Pre-amp: \(model.settings.preAmpDb, specifier: "%.1f") dB",
                        value: Binding(
                            get: { model.settings.preAmpDb },
                            set: { v in model.edit { $0.preAmpDb = v } }
                        ),
                        in: -15...15,
                        step: 0.5
                    )
                }
            } header: {
                Text("Loudness")
            } footer: {
                Text("Applies the gain written into the file's tags. Per album keeps the relative loudness within a record.")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
            }
        }
        .formStyle(.grouped)
    }
}

// MARK: - Radio

private struct RadioSettings: View {
    @Bindable var model: SettingsModel

    var body: some View {
        Form {
            Section {
                Stepper(
                    "Keep \(model.settings.radioLookahead) tracks queued ahead",
                    value: Binding(
                        get: { Int(model.settings.radioLookahead) },
                        set: { v in model.edit { $0.radioLookahead = UInt32(v) } }
                    ),
                    in: 1...25
                )
                Stepper(
                    "Add \(model.settings.radioBatchSize) at a time",
                    value: Binding(
                        get: { Int(model.settings.radioBatchSize) },
                        set: { v in model.edit { $0.radioBatchSize = UInt32(v) } }
                    ),
                    in: 1...25
                )
            } header: {
                Text("Topping up")
            } footer: {
                Text("Radio adds tracks when the queue runs shorter than this.")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
            }

            Section {
                Slider(
                    value: Binding(
                        get: { model.settings.radioDiscoveryWeight },
                        set: { v in model.edit { $0.radioDiscoveryWeight = v } }
                    ),
                    in: 0...1
                ) {
                    Text("Discovery")
                } minimumValueLabel: {
                    Text("Familiar").font(.caption)
                } maximumValueLabel: {
                    Text("New").font(.caption)
                }
            } header: {
                Text("What it picks")
            } footer: {
                Text("Higher favours tracks you have not played, or have not played in a long time.")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
            }
        }
        .formStyle(.grouped)
    }
}
