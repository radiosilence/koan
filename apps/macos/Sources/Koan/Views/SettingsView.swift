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
    @Environment(AppState.self) private var app
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
            content()
                .navigationTitle(title)
                #if os(tvOS)
                .roomBackground()
                #endif
        } label: {
            #if os(tvOS)
            // The symbols are of different widths; at television size a
            // label's own spacing lets the wide ones touch their titles.
            HStack(spacing: 24) {
                Image(systemName: symbol).frame(width: 56)
                Text(title)
            }
            #else
            KoanLabel(title, icon: symbol)
            #endif
        }
        .listLink()
    }
    #endif

    var body: some View {
        Group {
            if let model {
                #if os(macOS)
                // The panes side by side in a window sized to hold the largest
                // of them, which is what a settings window is on macOS.
                TabView {
                    LibrarySettings(model: model)
                        .tabItem { Label("Library", systemImage: "music.note.house") }
                    RemoteSettings(model: model)
                        .tabItem { Label("Server", systemImage: "server.rack") }
                    PlaybackSettings(model: model)
                        .tabItem { Label("Playback", systemImage: "hifispeaker") }
                    EqSettings()
                        .tabItem { Label("EQ", systemImage: "slider.vertical.3") }
                    DevicesSettings(model: model)
                        .tabItem { Label("Devices", systemImage: "laptopcomputer.and.iphone") }
                    AppearanceSettings()
                        .tabItem { Label("Appearance", systemImage: "paintpalette") }
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
                    // Each pane carries the status line too: a pane pushed
                    // over the list hides the list's, and with it the reason a
                    // sign-in failed.
                    pane("Server", "server.rack") {
                        RemoteSettings(model: model)
                            .safeAreaInset(edge: .bottom) { StatusLine(model: model) }
                    }
                    pane("Playback", "hifispeaker") {
                        PlaybackSettings(model: model)
                            .safeAreaInset(edge: .bottom) { StatusLine(model: model) }
                    }
                    pane("EQ", "slider.vertical.3") {
                        EqSettings()
                            .safeAreaInset(edge: .bottom) { StatusLine(model: model) }
                    }
                    pane("Devices", "laptopcomputer.and.iphone") {
                        DevicesSettings(model: model)
                            .safeAreaInset(edge: .bottom) { StatusLine(model: model) }
                    }
                    pane("Appearance", "paintpalette") {
                        AppearanceSettings()
                    }
                    Section {} footer: {
                        Text(AppVersion.text)
                            .koanText(.fine, .muted)
                            .frame(maxWidth: .infinity)
                    }
                }
                .navigationTitle(KoanTheme.label("Settings"))
                .safeAreaInset(edge: .bottom) { StatusLine(model: model) }
                #endif
            } else {
                ProgressView()
            }
        }
        // The settings window: tall enough for the longest pane on a 1440×900
        // screen, resizable, and kept at whatever size it was last given. A
        // phone gets whatever it has.
        #if os(macOS)
        .frame(minWidth: 600, idealWidth: 820, maxWidth: .infinity, minHeight: 480, idealHeight: 780, maxHeight: .infinity)
        .background(SettingsFrameAutosave())
        #endif
        #if os(macOS)
        .modifier(DspImportPrompts(dsp: app.dsp))
        #endif
        .task {
            if model == nil {
                model = await SettingsModel(engine: library.engine, activity: activity, art: library.art)
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
                KoanLabel(error, icon: "exclamationmark.triangle.fill")
                    .koanText(.fine, .bad)
            } else if let result = model.lastResult {
                KoanLabel(result, icon: "checkmark.circle")
            } else {
                Text(" ")
            }
        }
        .koanText(.fine, .muted)
        .lineLimit(2)
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 18)
        .padding(.vertical, 8)
        #if os(tvOS)
        .background(.regularMaterial)
        #else
        .background(.bar)
        #endif
    }
}

extension SettingsModel {
    /// A binding to one field, committed through `edit` on every change.
    func binding<Value>(_ field: WritableKeyPath<KoanFFI.Settings, Value> & Sendable) -> Binding<Value> {
        Binding(
            get: { self.settings[keyPath: field] },
            set: { value in self.edit { $0[keyPath: field] = value } }
        )
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
        KoanForm {
            Section {
                if model.settings.libraryFolders.isEmpty {
                    Text("No folders yet — kōan has nothing to scan.")
                        .koanText(.meta, .muted)
                }
                ForEach(model.settings.libraryFolders, id: \.path) { folder in
                    HStack {
                        Text(folder.path)
                            .koanText(.meta)
                            .monospaced()
                            .lineLimit(1)
                            .truncationMode(.head)
                            .help(folder.path)
                        Spacer(minLength: 8)
                        Text(Format.count(Int64(folder.tracks), "track"))
                            .koanText(.fine, .muted)
                            .monospacedDigit()
                        Button {
                            removing = folder
                        } label: {
                            Image(systemName: "minus.circle")
                        }
                        .koanButton(.icon)
                        .help("Stop scanning this folder")
                    }
                }
                // Adding a folder starts a scan, so it waits for the one running.
                Button("Add Folder…") { choosingFolder = true }
                    .koanButton(.secondary)
                    .disabled(activity.conflicts(with: .localLibrary))
            } header: {
                KoanSectionHeader("Folders")
            } footer: {
                Text("Removing a folder stops it being scanned. It does not delete anything.")
                    .koanText(.fine, .muted)
            }

            Section {
                HStack {
                    Button("Scan") { model.scan() }
                        .koanButton(.secondary)
                    Button("Rescan Everything") { model.scan(force: true) }
                        .koanButton(.secondary)
                        .help("Re-read every file's tags, ignoring the scan cache")
                }
                .rowButtons()
                // One pass over your files at a time. A sync or a download
                // clear is welcome to run alongside; another scan, a drop or a
                // file move would be reading and writing the same things.
                .disabled(activity.conflicts(with: .localLibrary))
            } header: {
                KoanSectionHeader("Scan")
            } footer: {
                if activity.conflicts(with: .localLibrary) {
                    Text("Waiting for the task that is reading your files to finish.")
                        .koanText(.fine, .muted)
                }
            }

            Section {
                // Empties every table, so it waits for everything.
                Button("Clear Library Index…", role: .destructive) {
                    confirmingRebuild = true
                }
                .koanButton(.secondary)
                .disabled(activity.conflicts(with: .wholeLibrary))
            } header: {
                KoanSectionHeader("Rebuild")
            } footer: {
                Text("""
                    Forgets every artist, album and track so the next scan builds \
                    them again from your files. Favourites survive — they are kept \
                    against file paths. Lyrics, play counts and audio analysis do \
                    not; they are tied to rows that will not exist.
                    """)
                    .koanText(.fine, .muted)
            }
        }
        .koanSheet()
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
            Text("Your files are not touched either way. Keeping them leaves records in the library that kōan will not scan again.")
        }
        .filePicker(
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
    @Environment(AppState.self) private var state
    @Environment(EngineMirror.self) private var mirror
    @State private var url = ""
    @State private var username = ""
    @State private var confirmingSignOut = false
    @State private var copiedServer = false
    @State private var changingPassword = false
    @State private var currentPassword = ""
    @State private var newPassword = ""
    /// What the server holds, while asking before it replaces this queue.
    @State private var replacingQueue: ServerQueue?
    /// The cache limit as typed, committed whole: "5" on the way to "50GB" is
    /// not a limit anyone set.
    @State private var cacheLimit: String?
    @FocusState private var cacheLimitFocused: Bool

    private func join(_ text: String) {
        guard let invite = state.engine.parseInvite(link: text) else {
            model.report("That isn't a kōan invite.")
            return
        }
        url = ""
        Task { await state.offer(invite) }
    }

    var body: some View {
        KoanForm {
            if model.settings.remoteSignedIn {
                Section {
                    #if os(tvOS)
                    LabeledContent("Server", value: model.settings.remoteUrl)
                    #else
                    // The address is what another device or app asks for, so a
                    // tap copies it.
                    Button {
                        Pasteboard.write(text: model.settings.remoteUrl)
                        copiedServer = true
                    } label: {
                        LabeledContent("Server", value: copiedServer ? "Copied" : model.settings.remoteUrl)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .help("Copy the server's address")
                    .task(id: copiedServer) {
                        guard copiedServer else { return }
                        try? await Task.sleep(for: .seconds(1.5))
                        copiedServer = false
                    }
                    #endif
                    LabeledContent("User", value: model.settings.remoteUsername)
                    LabeledContent(
                        "Tracks",
                        value: Format.count(Int64(model.settings.remoteTracks), "track")
                    )
                    if mirror.signInRefused {
                        KoanLabel(EngineMirror.signInRefusedDetail, icon: "exclamationmark.triangle")
                            .koanText(.meta, .bad)
                    }
                    HStack {
                        // Only the syncs wait on the database writer. Signing
                        // out is a config write, and greying it out while a
                        // sync runs strands you on a server you are trying to
                        // leave.
                        Button("Sync") { model.syncNow() }
                            .koanButton(.secondary)
                            .disabled(activity.conflicts(with: [.remoteTracks]))
                        #if !os(tvOS)
                        if mirror.offers(PasswordChange.extensionName) {
                            Button("Change Password…") { changingPassword = true }
                                .koanButton(.secondary)
                        }
                        #endif
                        Spacer()
                        Button("Sign Out", role: .destructive) { confirmingSignOut = true }
                            .koanButton(.secondary)
                    }
                    .rowButtons()
                } header: {
                    KoanSectionHeader("Signed in")
                }
                #if !os(tvOS)
                .alert("Change your password", isPresented: $changingPassword) {
                    SecureField("Current password", text: $currentPassword)
                    SecureField("New password", text: $newPassword)
                    Button("Change") {
                        let (current, new) = (currentPassword, newPassword)
                        currentPassword = ""
                        newPassword = ""
                        Task { _ = await model.changePassword(current: current, new: new) }
                    }
                    Button("Cancel", role: .cancel) {
                        currentPassword = ""
                        newPassword = ""
                    }
                } message: {
                    Text("This device stays signed in. Your other devices, and other apps using this account, will have to sign in again.")
                }
                #endif
                if mirror.offers(ApiKeysSettings.extensionName) {
                    ApiKeysSettings()
                }
                if mirror.offers(AssistantsSettings.extensionName) {
                    AssistantsSettings()
                }
                // Accounts and pairings are managed from a device with a keyboard.
                #if !os(tvOS)
                PeopleSettings(signedInAs: model.settings.remoteUsername)
                PairDevice()
                #endif
                ScrobblingSettings()
                ServerOffers()
            } else {
                Section {
                    // The prompt names the field: an iOS form shows only the
                    // prompt, so an example there leaves the field unlabelled.
                    LabeledContent("Server URL") {
                        TextField("Server URL", text: $url, prompt: Text("Server URL"))
                            .verbatimEntry(.url)
                            .accessibilityIdentifier("server-url")
                            .koanField()
                    }
                    LabeledContent("Username") {
                        TextField("Username", text: $username, prompt: Text("Username"))
                            .verbatimEntry()
                            .accessibilityIdentifier("username")
                            .koanField()
                    }
                    Picker("Sign in with", selection: $model.withApiKey) {
                        Text("Password").tag(false)
                        Text("API key").tag(true)
                    }.koanControl()
                    #if os(tvOS)
                    // Two choices side by side, rather than a page of their
                    // own to go into and come back from.
                    .pickerStyle(.segmented)
                    #endif
                    LabeledContent(model.withApiKey ? "API key" : "Password") {
                        SecureField(
                            model.withApiKey ? "API key" : "Password",
                            text: $model.password,
                            prompt: Text(model.withApiKey ? "API key" : "Password")
                        )
                        .verbatimEntry()
                        .accessibilityIdentifier("secret")
                        .koanField()
                    }
                    HStack {
                        Button("Sign In") { model.signIn(url: url, username: username) }
                            .koanButton(.primary)
                            .disabled(url.isEmpty || username.isEmpty || model.password.isEmpty)
                        Spacer()
                        #if !os(tvOS)
                        PasteButton(payloadType: String.self) { strings in
                            Task { @MainActor in join(strings.first ?? "") }
                        }
                        .labelStyle(.titleAndIcon)
                        #endif
                    }
                    .rowButtons()
                } header: {
                    KoanSectionHeader("Subsonic or Navidrome")
                } footer: {
                    Text("Paste an invite here, or into Server URL, and kōan fills in the rest. The account is checked against the server, then saved to config.local.toml, readable only by you.")
                        .koanText(.fine, .muted)
                }
                // An invite, or an address with the account in it, pasted
                // where the address goes.
                .onChange(of: url) { _, typed in
                    if state.engine.parseInvite(link: typed) != nil { join(typed) }
                }
            }

            #if os(iOS)
            // Offline is from a server: nothing to be offline from before
            // signing in, and the switch would sit between the form and its
            // button.
            if model.settings.remoteSignedIn {
            Section {
                Toggle("Offline mode", isOn: Binding(
                    get: { mirror.connection?.offlineManual ?? false },
                    set: { state.library.engine.setOffline(on: $0) }
                )).koanToggle()
            } footer: {
                Text("Shows only what is on this iPhone. It turns on by itself when your server cannot be reached, and off again when it can.")
                    .koanText(.fine, .muted)
            }
            }
            #endif

            Section {
                Toggle("Keep the library in sync", isOn: model.binding(\.autoSync)).koanToggle()
                if model.settings.autoSync {
                    Picker("Every", selection: model.binding(\.autoSyncIntervalMins)) {
                        Text("Startup only").tag(UInt64(0))
                        Text("15 minutes").tag(UInt64(15))
                        Text("Hour").tag(UInt64(60))
                        Text("6 hours").tag(UInt64(360))
                        Text("Day").tag(UInt64(1440))
                    }.koanControl()
                }
            } header: {
                KoanSectionHeader("Automatic sync")
            } footer: {
                Text("Each sync asks the server only for what changed since the last.")
                    .koanText(.fine, .muted)
            }

            if model.settings.remoteSignedIn {
            Section {
                Toggle("Keep the queue on the server", isOn: Binding(
                    get: { model.settings.playQueue },
                    set: { on in
                        Task {
                            // Turning it on takes the server's queue in place of
                            // this one, so say so first when there is one.
                            if on, let saved = await model.serverQueue(), saved.savedTracks > 0 {
                                replacingQueue = saved
                            } else {
                                await model.setServerQueue(on)
                            }
                        }
                    }
                )).koanToggle()
            } header: {
                KoanSectionHeader("Play queue")
            } footer: {
                Text("Saves this device's queue to your account on the server, where other apps can pick it up, and picks up a queue another app saved there when kōan starts. Moving music between kōan devices does not need it.")
                    .koanText(.fine, .muted)
            }
            }

            Section {
                #if os(tvOS)
                // tvOS has no stepper.
                Picker("Parallel downloads", selection: Binding(
                    get: { Int(model.settings.downloadWorkers) },
                    set: { v in model.edit { $0.downloadWorkers = UInt32(v) } }
                )) {
                    ForEach(1...16, id: \.self) { Text("\($0)").tag($0) }
                }.koanControl()
                #else
                Stepper(
                    "Parallel downloads: \(model.settings.downloadWorkers)",
                    value: Binding(
                        get: { Int(model.settings.downloadWorkers) },
                        set: { v in model.edit { $0.downloadWorkers = UInt32(v) } }
                    ),
                    in: 1...16
                ).koanControl()
                #endif
                LabeledContent("Cache limit") {
                    TextField("Cache limit", text: Binding(
                        get: { cacheLimit ?? model.settings.cacheLimit },
                        set: { cacheLimit = $0 }
                    ), prompt: Text("e.g. 50GB — blank for no limit"))
                    .verbatimEntry()
                    .focused($cacheLimitFocused)
                    .onSubmit(commitCacheLimit)
                    .onChange(of: cacheLimitFocused) { _, focused in
                        if !focused { commitCacheLimit() }
                    }
                    .koanField()
                }
                LabeledContent("Using") {
                    HStack {
                        Text(Format.bytes(Int64(model.settings.cacheBytes)))
                        Button("Clear") { model.clearCache() }
                            .koanButton(.text)
                            .disabled(activity.conflicts(with: [.downloads]))
                    }
                }
            } header: {
                KoanSectionHeader("Downloads")
            }
        }
        .koanSheet()
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
            Button("Sign Out and Keep Its Tracks") {
                model.signOut(forgetTracks: false)
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Tracks you also have as local files are kept either way. Keeping the rest leaves records in the library that cannot be played until you sign in again.")
        }
        .confirmationDialog(
            "Replace this queue?",
            isPresented: Binding(
                get: { replacingQueue != nil },
                set: { if !$0 { replacingQueue = nil } }
            ),
            titleVisibility: .visible,
            presenting: replacingQueue
        ) { _ in
            Button("Replace With the Server's Queue", role: .destructive) {
                Task { await model.setServerQueue(true) }
            }
            Button("Cancel", role: .cancel) {}
        } message: { saved in
            Text("Your server has a queue of \(saved.savedTracks) \(saved.savedTracks == 1 ? "track" : "tracks") saved by \(saved.savedBy). Keeping the queue on the server replaces the one on this device with it.")
        }
    }

    private func commitCacheLimit() {
        guard let draft = cacheLimit else { return }
        cacheLimit = nil
        if draft != model.settings.cacheLimit { model.edit { $0.cacheLimit = draft } }
    }
}

// MARK: - Playback

private struct PlaybackSettings: View {
    @Bindable var model: SettingsModel
    @Environment(PlayerModel.self) private var player

    var body: some View {
        KoanForm {
            Section {
                Picker("Output", selection: Binding(
                    get: { player.currentDevice ?? "" },
                    set: { player.setDevice($0.isEmpty ? nil : $0) }
                )) {
                    Text("System Default").tag("")
                    ForEach(player.devices, id: \.name) { device in
                        Text(device.name).tag(device.name)
                    }
                }.koanControl()
            } header: {
                KoanSectionHeader("Device")
            } footer: {
                Text("kōan asks the device to run at the source's sample rate, so nothing is resampled unless the device refuses.")
                    .koanText(.fine, .muted)
            }

            Section {
                Toggle("Fade on pause", isOn: model.binding(\.fadeOnPause)).koanToggle()
            } header: {
                KoanSectionHeader("Transport")
            } footer: {
                Text("Pause and resume ramp the volume over a moment instead of cutting.")
                    .koanText(.fine, .muted)
            }

            Section {
                Picker("ReplayGain", selection: model.binding(\.replaygain)) {
                    Text("Off").tag("off")
                    Text("Per track").tag("track")
                    Text("Per album").tag("album")
                }.koanControl()
                if model.settings.replaygain != "off" {
                    #if os(tvOS)
                    Picker("Pre-amp", selection: model.binding(\.preAmpDb)) {
                        ForEach(Array(stride(from: -15.0, through: 15.0, by: 0.5)), id: \.self) { db in
                            Text("\(db, specifier: "%.1f") dB").tag(db)
                        }
                    }.koanControl()
                    #else
                    Stepper(
                        "Pre-amp: \(model.settings.preAmpDb, specifier: "%.1f") dB",
                        value: model.binding(\.preAmpDb),
                        in: -15...15,
                        step: 0.5
                    ).koanControl()
                    #endif
                }
            } header: {
                KoanSectionHeader("Loudness")
            } footer: {
                Text("Applies the gain written into the file's tags. Per album keeps the relative loudness within a record.")
                    .koanText(.fine, .muted)
            }
        }
        .koanSheet()
    }
}

/// EQ for the output in use: what its profile does to the sound, drawn,
/// then the profiles and where they come from. A page of its own: the graph
/// wants the room, and a correction is chosen, shaped and checked here.
struct EqSettings: View {
    @Environment(AppState.self) private var app
    @State private var response: DspResponse?
    @State private var detail: DspProfileDetail?

    private var active: String? { app.dsp.overview?.active }

    var body: some View {
        KoanForm {
            if let active, let response, let detail {
                Section {
                    EqGraph(response: response, handles: BandTable.handles(detail.bands)) { index, hz, db in
                        let b = detail.bands[index]
                        app.dsp.setBand(active, index, kind: b.kind, freq: hz, gain: db, q: b.q)
                    }
                } header: {
                    Text(active)
                }
                BandTable(dsp: app.dsp, profile: active, bands: detail.bands)
            }
            DspSettings()
        }
        .koanSheet()
        .task(id: "\(active ?? "")\u{0}\(app.dsp.version)") {
            response = if let active { await app.dsp.response(active) } else { nil }
            detail = if let active { await app.dsp.detail(active) } else { nil }
        }
    }
}

/// Correction for the output in use: a profile of bands, impulse responses or
/// both, imported from what other tools write.
struct DspSettings: View {
    @Environment(AppState.self) private var app
    @State private var importing = false
    @State private var findingAutoEq = false
    /// What Find in AutoEQ opens searching for: empty from its button, a
    /// model from an offer for the output in use.
    @State private var findQuery = ""
    /// The profile whose page is open, on the Mac, where settings has no
    /// navigation stack to push it onto.
    @State private var showing: String?

    var body: some View {
        let dsp = app.dsp
        Section {
            if let o = dsp.overview {
                Toggle("Process audio", isOn: Binding(
                    get: { o.enabled },
                    set: { dsp.setEnabled($0) }
                )).koanToggle()
                if let device = o.device, !o.profiles.isEmpty {
                    Picker("Profile for \(dsp.label(device))", selection: Binding(
                        get: { o.active ?? "" },
                        set: { dsp.use($0.isEmpty ? nil : $0) }
                    )) {
                        Text("None").tag("")
                        ForEach(o.profiles, id: \.name) { p in
                            Text(p.name).tag(p.name)
                        }
                    }.koanControl()
                    .disabled(!o.enabled)
                }
                #if !os(tvOS)
                if let offer = dsp.suggestion, o.device != nil {
                    AutoEqSuggestion(offer: offer, dsp: dsp) { query in
                        findQuery = query
                        findingAutoEq = true
                    }
                }
                #endif
                ForEach(o.profiles, id: \.name) { p in
                    #if os(iOS)
                    NavigationLink {
                        DspProfilePage(dsp: dsp, name: p.name)
                    } label: {
                        ProfileRow(profile: p, active: o.active == p.name)
                    }
                    .swipeActions {
                        Button("Delete", role: .destructive) { dsp.remove(p.name) }
                    }
                    #else
                    Button {
                        showing = p.name
                    } label: {
                        ProfileRow(profile: p, active: o.active == p.name)
                    }
                    .buttonStyle(.plain)
                    .contextMenu {
                        Button("Delete", role: .destructive) { dsp.remove(p.name) }
                    }
                    #endif
                }
            }
            // Profiles come in as files, and a television has none: they are
            // imported on another device, and the TV picks them by output.
            #if !os(tvOS)
            Button("Import…") { importing = true }
                .koanButton(.secondary)
            Button("Find in AutoEQ…") {
                findQuery = ""
                findingAutoEq = true
            }
            .koanButton(.secondary)
            #endif
            if let error = dsp.lastError {
                Text(error)
                    .koanText(.fine, .bad)
            }
        } header: {
            KoanSectionHeader("EQ and convolution")
        } footer: {
            Text("AutoEQ and Equalizer APO text, impulse WAVs, Roon zips, Convolver .cfg and CamillaDSP configs, or a headphone found in AutoEQ by name. Importing into a profile of the same name adds to it. An output without a profile plays untouched.")
                .koanText(.fine, .muted)
        }
        .filePicker(
            isPresented: $importing,
            allowedContentTypes: [.item, .folder],
            allowsMultipleSelection: true
        ) { result in
            if case let .success(urls) = result, !urls.isEmpty {
                dsp.importFiles(urls)
            }
        }
        .task { dsp.reload() }
        #if !os(tvOS)
        .sheet(isPresented: $findingAutoEq) {
            AutoEqSearch(dsp: dsp, query: findQuery)
        }
        #endif
        #if os(macOS)
        .sheet(item: Binding(
            get: { showing.map(ShownProfile.init) },
            set: { showing = $0?.name }
        )) { shown in
            NavigationStack {
                DspProfilePage(dsp: dsp, name: shown.name)
                    .toolbar {
                        ToolbarItem(placement: .confirmationAction) {
                            Button("Done") { showing = nil }
                        }
                    }
            }
            .frame(minWidth: 480, minHeight: 440)
        }
        #endif
    }
}

#if !os(tvOS)
/// The output in use, recognised by its name as a headphone AutoEQ has
/// measured, or roughly so: its profile, or a search for its model to pick
/// the right one from. Offered once, quietly; nothing is applied until asked.
private struct AutoEqSuggestion: View {
    let offer: AutoEqOffer
    let dsp: DspModel
    let find: (String) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            switch offer {
            case let .profile(entry):
                Text("AutoEQ has a profile for \(entry.name). Use it?")
                    .koanText(.body, .muted)
                HStack {
                    Button("Use") { dsp.installAutoEq(entry) }
                        .koanButton(.primary)
                    dismiss
                    Spacer()
                }
            case let .search(query):
                HStack {
                    Button("Find \(query) in AutoEQ…") { find(query) }
                        .koanButton(.secondary)
                    dismiss
                    Spacer()
                }
            }
        }
        .koanButton(.text)
        .koanText(.meta)
    }

    private var dismiss: some View {
        Button("Not for This Device") { dsp.dismissSuggestion() }
            .koanButton(.text)
    }
}

/// AutoEQ's results by headphone name. Choosing one installs it and plays the
/// output in use through it.
private struct AutoEqSearch: View {
    let dsp: DspModel
    @Environment(\.dismiss) private var dismiss
    @State private var query: String

    init(dsp: DspModel, query: String = "") {
        self.dsp = dsp
        _query = State(initialValue: query)
    }

    var body: some View {
        NavigationStack {
            Group {
                if query.isEmpty {
                    // Nothing typed: the makers, each opening its models.
                    List(dsp.autoEqMakers, id: \.name) { maker in
                        NavigationLink {
                            AutoEqModels(dsp: dsp, maker: maker.name) { dismiss() }
                        } label: {
                            LabeledContent(maker.name, value: "\(maker.results)")
                        }
                    }
                    .task { await dsp.loadAutoEqMakers() }
                } else {
                    List(dsp.autoEqResults, id: \.profileName) { entry in
                        AutoEqRow(entry: entry) {
                            dsp.installAutoEq(entry)
                            dismiss()
                        }
                    }
                    .overlay {
                        if dsp.autoEqResults.isEmpty {
                            ContentUnavailableView.search(text: query)
                        }
                    }
                }
            }
            .searchable(text: $query, prompt: "Headphone")
            .navigationTitle(KoanTheme.label("AutoEQ"))
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
            }
            // Debounced: a search runs once typing pauses, not per keystroke.
            .task(id: query) {
                try? await Task.sleep(for: .milliseconds(250))
                guard !Task.isCancelled else { return }
                await dsp.searchAutoEq(query)
            }
        }
        #if os(macOS)
        .frame(minWidth: 420, minHeight: 460)
        #endif
    }
}
#endif

#if !os(tvOS)
/// One AutoEQ result: the headphone, and who measured it.
private struct AutoEqRow: View {
    let entry: AutoEqEntry
    let choose: () -> Void

    var body: some View {
        Button(action: choose) {
            VStack(alignment: .leading, spacing: 2) {
                Text(entry.name)
                Text(entry.measuredBy)
                    .koanText(.fine, .muted)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

/// A maker's results, by model; where several people measured one, the one
/// AutoEQ recommends comes first. Choosing one installs it and closes the
/// search.
private struct AutoEqModels: View {
    let dsp: DspModel
    let maker: String
    let done: () -> Void
    @State private var models: [AutoEqEntry] = []

    var body: some View {
        List(models, id: \.profileName) { entry in
            AutoEqRow(entry: entry) {
                dsp.installAutoEq(entry)
                done()
            }
        }
        .navigationTitle(maker)
        .task { models = await dsp.autoEqModels(maker) }
    }
}
#endif

private struct ShownProfile: Identifiable {
    let name: String
    var id: String { name }
}

/// A profile in the list: its name, what it holds, and a tick on the one the
/// output in use plays through.
private struct ProfileRow: View {
    let profile: DspProfileSummary
    let active: Bool

    var body: some View {
        HStack {
            VStack(alignment: .leading, spacing: 2) {
                Text(profile.name)
                if let problem = profile.problem {
                    Text(problem)
                        .koanText(.fine, .bad)
                } else {
                    Text(DspModel.describe(profile))
                        .koanText(.fine, .muted)
                }
            }
            Spacer()
            if active {
                Image(systemName: "checkmark")
                    .koanText(.body, .accent)
                    .accessibilityLabel("In use")
            }
        }
        .contentShape(Rectangle())
    }
}

/// The questions an import can stop on — what rate bare coefficients are at —
/// and the offer of what it made for the output in use. On the settings page,
/// and on iOS over everything, since a share can arrive anywhere.
struct DspImportPrompts: ViewModifier {
    let dsp: DspModel

    func body(content: Content) -> some View {
        content
            .confirmationDialog(
                "What sample rate is it at?",
                isPresented: Binding(
                    get: { dsp.needsRate != nil },
                    set: { if !$0 { dsp.needsRate = nil } }
                ),
                titleVisibility: .visible
            ) {
                ForEach(DspModel.rates, id: \.self) { rate in
                    Button("\(DspModel.khz(rate)) kHz") { dsp.retry(rate: rate) }
                }
                Button("Cancel", role: .cancel) { dsp.needsRate = nil }
            } message: {
                Text("These coefficients carry no rate of their own. Use the one the filter was designed at.")
            }
            .alert(
                "Imported \(dsp.imported ?? "")",
                isPresented: Binding(
                    get: { dsp.imported != nil },
                    set: { if !$0 { dsp.imported = nil } }
                ),
                presenting: dsp.imported
            ) { name in
                if let device = dsp.overview?.device, dsp.overview?.active != name {
                    Button("Use for" + " \(device)") { dsp.use(name) }
                }
                Button("Done", role: .cancel) {}
            } message: { _ in
                if let device = dsp.overview?.device, dsp.overview?.active == nil {
                    Text("\(device) plays untouched until it has a profile.")
                }
            }
    }
}

/// Signing in a device that has no keyboard, by the code it shows. Offered
/// where the server lists `koanPair`.
private struct PairDevice: View {
    @Environment(AppState.self) private var state
    @Environment(EngineMirror.self) private var mirror
    @State private var code = ""

    var body: some View {
        if mirror.connection?.pairing == true {
            Section {
                HStack {
                    TextField("Code", text: $code, prompt: Text("XXXX-XXXX"))
                        .verbatimEntry()
                        .onSubmit(approve)
                        .koanField()
                    Button("Approve", action: approve)
                        .koanButton(.primary)
                        .disabled(code.trimmingCharacters(in: .whitespaces).isEmpty)
                }
            } header: {
                KoanSectionHeader("Pair a device")
            } footer: {
                Text("A television or another device without a keyboard shows a code while it waits. Enter it here to sign it in as you, with a key of its own that can be revoked on the server.")
                    .koanText(.fine, .muted)
            }
        }
    }

    private func approve() {
        let typed = code.trimmingCharacters(in: .whitespaces)
        guard !typed.isEmpty else { return }
        code = ""
        Task { await state.offerPairing(typed) }
    }
}

/// The account's scrobbling, which the server does: the token is handed over
/// once and never comes back. Offered where the server lists
/// `koanScrobbling`. A television shows where things stand and leaves the
/// typing to a device with a keyboard.
private struct ScrobblingSettings: View {
    @Environment(AppState.self) private var state
    @Environment(EngineMirror.self) private var mirror
    @State private var connection: ScrobblingConnection?
    @State private var loaded = false
    /// The server could not say whether the account is connected, so neither
    /// state is shown.
    @State private var statusFailed = false
    @State private var token = ""
    @State private var busy = false
    @State private var error: String?

    var body: some View {
        if mirror.connection?.scrobbling == true {
            Section {
                if let c = connection {
                    Text("Scrobbling to ListenBrainz as \(c.account)")
                    if let refused = c.error {
                        Label(refused, systemImage: "exclamationmark.triangle")
                            .foregroundStyle(.orange)
                        Text("Disconnect, then connect again with a current token. Plays recorded meanwhile are kept and sent.")
                            .foregroundStyle(.secondary)
                    } else if c.pending > 0 {
                        Text(Format.count(c.pending, "play") + " waiting to be sent")
                            .foregroundStyle(.secondary)
                    }
                    #if !os(tvOS)
                    Button("Disconnect", role: .destructive, action: disconnect)
                        .disabled(busy)
                    #endif
                } else if !loaded {
                    Text("Checking…").foregroundStyle(.secondary)
                } else if statusFailed {
                    Button("Try Again") { Task { await load() } }
                } else {
                    #if os(tvOS)
                    Text("Not connected. Connect ListenBrainz from kōan on a phone or Mac.")
                        .foregroundStyle(.secondary)
                    #else
                    SecureField("User token", text: $token, prompt: Text("ListenBrainz user token"))
                        .verbatimEntry()
                        .onSubmit(connect)
                    HStack {
                        Link("Find your token", destination: URL(string: "https://listenbrainz.org/settings/")!)
                        Spacer()
                        Button("Connect", action: connect)
                            .disabled(busy || token.trimmingCharacters(in: .whitespaces).isEmpty)
                    }
                    .rowButtons()
                    #endif
                }
                if let error {
                    Label(error, systemImage: "exclamationmark.triangle")
                        .foregroundStyle(.red)
                }
            } header: {
                Text("Scrobbling")
            } footer: {
                Text("The server sends what you play to ListenBrainz, from every app signed in as you, your history included when you connect. The token is kept on the server.")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
            }
            .task(id: mirror.connection?.scrobbling) { await load() }
        }
    }

    private func load() async {
        do {
            connection = try await state.engine.scrobblingStatus()
            statusFailed = false
            error = nil
        } catch {
            statusFailed = true
            self.error = SettingsModel.describe(error)
        }
        loaded = true
    }

    private func connect() {
        let typed = token.trimmingCharacters(in: .whitespaces)
        guard !typed.isEmpty, !busy else { return }
        busy = true
        Task {
            do {
                connection = try await state.engine.connectScrobbling(token: typed)
                token = ""
                error = nil
            } catch {
                self.error = SettingsModel.describe(error)
            }
            busy = false
        }
    }

    private func disconnect() {
        busy = true
        Task {
            do {
                try await state.engine.disconnectScrobbling()
                connection = nil
                error = nil
            } catch {
                self.error = SettingsModel.describe(error)
            }
            busy = false
        }
    }
}

// MARK: - Server

/// What the server said it is when koan signed in, and what that turns on.
/// koan's own features are OpenSubsonic extensions, so a server lists them the
/// same way it lists any other.
private struct ServerOffers: View {
    @Environment(EngineMirror.self) private var mirror

    var body: some View {
        Section {
            if let c = mirror.connection, c.serverKind != nil || c.openSubsonic {
                LabeledContent("Server", value: server(c))
                LabeledContent("OpenSubsonic", value: c.openSubsonic ? "Yes" : "No")
                LabeledContent("Your devices", value: devices(c))
                if !c.extensions.isEmpty {
                    #if os(tvOS)
                    extensionList(c.extensions)
                    #else
                    DisclosureGroup("Extensions (\(c.extensions.count))") {
                        extensionList(c.extensions)
                    }
                    #endif
                }
            } else {
                Text("Not reached yet")
                    .koanText(.body, .muted)
            }
        } header: {
            KoanSectionHeader("What the server offers")
        } footer: {
            Text("Asked when kōan signs in and whenever its link to the server reconnects. Features beyond Subsonic are used only where the server lists them.")
                .koanText(.fine, .muted)
        }
    }

    private func extensionList(_ extensions: [ServerExtension]) -> some View {
        ForEach(extensions, id: \.name) { e in
            LabeledContent(e.name, value: e.versions.map { "v\($0)" }.joined(separator: ", "))
                .koanText(.meta)
        }
    }

    private func server(_ c: ConnectionInfo) -> String {
        let name = switch c.serverKind {
        case "koan": "kōan"
        case let kind?: kind.prefix(1).uppercased() + kind.dropFirst()
        case nil: "Subsonic"
        }
        return [name, c.serverVersion].compactMap { $0 }.joined(separator: " ")
    }

    private func devices(_ c: ConnectionInfo) -> String {
        guard c.devices else { return "Not offered" }
        return c.linked ? "Connected" : "Offered, not connected"
    }
}

// MARK: - Devices

/// Being found and controlled by koan apps on the same network, and reaching
/// devices on networks that do not announce them.
private struct DevicesSettings: View {
    @Bindable var model: SettingsModel
    @Environment(EngineMirror.self) private var mirror
    #if os(macOS)
    @Environment(AppState.self) private var app
    @Environment(\.openWindow) private var openWindow
    #endif
    @State private var address = ""
    @State private var grantee = ""
    @State private var shareError: String?

    var body: some View {
        KoanForm {
            Section {
                Toggle("Discoverable on this network", isOn: model.binding(\.devicesDiscoverable)).koanToggle()
                Picker("Devices on this network", selection: model.binding(\.devicesNearbyControl)) {
                    Text("Full control").tag("full")
                    Text("Playback only").tag("playback")
                }.koanControl()
                if let port = mirror.connection?.listeningPort {
                    LabeledContent("Listening on port", value: String(port))
                }
                if mirror.connection?.localNetworkBlocked == true {
                    KoanLabel(LocalNetwork.blocked, icon: "wifi.exclamationmark")
                    .koanText(.meta, .bad)
                }
            } header: {
                KoanSectionHeader("This device")
            } footer: {
                Text("Any kōan app on this network can then see what is playing here and control it, whoever is signed in there: with Full control, the output, preset and volume too, and move the music here or away; with Playback only, play and the queue. Neither reaches your library, playlists or history. Choose Playback only on a network you share with strangers. Your own devices reach each other through your server either way.")
                    .koanText(.fine, .muted)
            }

            #if os(macOS)
            background
            #endif

            Section {
                ForEach(model.settings.devicesAddresses, id: \.self) { addr in
                    HStack {
                        Text(addr).koanText(.meta).monospaced()
                        Spacer()
                        Button("Remove", role: .destructive) {
                            model.edit { $0.devicesAddresses.removeAll { $0 == addr } }
                        }
                        .koanButton(.text)
                    }
                }
                HStack {
                    TextField("Address", text: $address, prompt: Text("host or host:port"))
                        .verbatimEntry(.url)
                        .onSubmit(add)
                        .koanField()
                    Button("Add", action: add)
                        .koanButton(.secondary)
                        .disabled(address.trimmingCharacters(in: .whitespaces).isEmpty)
                }
            } header: {
                KoanSectionHeader("Devices by address")
            } footer: {
                Text("For networks that do not announce devices, such as a tailnet. The port is 5626 unless the other device says otherwise.")
                    .koanText(.fine, .muted)
            }

            if mirror.connection?.sharing == true {
                Section {
                    ForEach(mirror.connection?.sharedWith ?? [], id: \.self) { account in
                        HStack {
                            Text(account)
                            Spacer()
                            Button("Stop sharing", role: .destructive) { share(account, allow: false) }
                                .koanButton(.text)
                        }
                    }
                    HStack {
                        TextField("Account", text: $grantee, prompt: Text("Their username on this server"))
                            .verbatimEntry()
                            #if os(macOS)
                            .textInputSuggestions {
                                ForEach(suggestions, id: \.self) { account in
                                    Text(account).textInputCompletion(account)
                                }
                            }
                            #endif
                            .onSubmit { share(grantee, allow: true) }
                            .koanField()
                        Button("Share") { share(grantee, allow: true) }
                            .koanButton(.secondary)
                            .disabled(grantee.trimmingCharacters(in: .whitespaces).isEmpty)
                    }
                    #if !os(macOS)
                    // iOS has no suggestions on a text field: the accounts
                    // matching what is typed, as rows to tap.
                    if !grantee.trimmingCharacters(in: .whitespaces).isEmpty {
                        ForEach(suggestions.filter { $0 != grantee }.prefix(5), id: \.self) { account in
                            Button(account) { grantee = account }
                        }
                    }
                    #endif
                    // Ours if it could not be sent; the server's if it refused.
                    if let error = shareError ?? mirror.connection?.shareError {
                        Text(error).koanText(.fine, .bad)
                    }
                } header: {
                    KoanSectionHeader("Shared with other accounts")
                } footer: {
                    Text("From any network, they can see what this device is playing and control its playback as on your own network: play, pause, skip, the queue, the output, preset and volume, and moving the music here or to their own devices. Each does it as their own account: nothing of your library, playlists, favourites or history, and nothing of your settings beyond what is playing and where.")
                        .koanText(.fine, .muted)
                }
            }
        }
        .koanSheet()
    }

    /// The server's accounts matching what is typed, not shared with yet.
    private var suggestions: [String] {
        let shared = Set(mirror.connection?.sharedWith ?? [])
        let typed = grantee.trimmingCharacters(in: .whitespaces).lowercased()
        return (mirror.connection?.shareAccounts ?? []).filter {
            !shared.contains($0) && (typed.isEmpty || $0.lowercased().hasPrefix(typed))
        }
    }

    private func share(_ account: String, allow: Bool) {
        let name = account.trimmingCharacters(in: .whitespaces)
        guard !name.isEmpty else { return }
        do {
            try model.shareDevice(with: name, allow: allow)
            shareError = nil
            if allow { grantee = "" }
        } catch {
            shareError = error.localizedDescription
        }
    }

    private func add() {
        let a = address.trimmingCharacters(in: .whitespaces)
        guard !a.isEmpty, !model.settings.devicesAddresses.contains(a) else { return }
        model.edit { $0.devicesAddresses.append(a) }
        address = ""
    }
}

// MARK: - Appearance

/// Not part of `config.toml`. How much the app draws is this machine's
/// business, and the TUI has none of it to draw, so it sits in defaults beside
/// the other view state rather than in the file the CLI shares.
private struct AppearanceSettings: View {
    @AppStorage("graphics") private var graphics = Graphics.full
    @Environment(AppearanceModel.self) private var appearance

    var body: some View {
        @Bindable var appearance = appearance
        KoanForm {
            Section {
                KoanSegmentedPicker(
                    options: [("kōan", true), ("System", false)],
                    selection: $appearance.koan,
                    title: "Theme"
                )
                // Follows the picker, not the theme drawn now: a change waits
                // for the next launch, and the icons are its to set.
                if appearance.koan {
                    Toggle("Show icons", isOn: $appearance.showIcons).koanToggle()
                }
            } header: {
                KoanSectionHeader("Theme")
            } footer: {
                Text("kōan is the site's look; System, the platform's own. A change of theme takes effect the next time kōan opens. Show icons puts icons beside the labels in the sidebar, the tabs and the buttons.")
                    .koanText(.fine, .muted)
            }
            Section {
                // Positioned by where a step sits in the list, not by its raw
                // value: the raw values are what is on disk and cannot be
                // reordered, and the cheapest step was added last. tvOS has
                // no slider; a picker in the same order stands in.
                #if os(tvOS)
                Picker("Level", selection: $graphics) {
                    ForEach(Graphics.allCases, id: \.self) { Text($0.label).tag($0) }
                }.koanControl()
                #else
                Slider(
                    value: Binding(
                        get: { Double(Graphics.allCases.firstIndex(of: graphics) ?? 0) },
                        set: { graphics = Graphics.allCases[Int($0.rounded())] }
                    ),
                    in: 0...Double(Graphics.allCases.count - 1),
                    step: 1
                ) {
                    Text("Level")
                } minimumValueLabel: {
                    Text(Graphics.allCases.first?.label ?? "").koanText(.fine, .muted)
                } maximumValueLabel: {
                    Text(Graphics.allCases.last?.label ?? "").koanText(.fine, .muted)
                }
                #endif
                Text("**\(graphics.label)** — \(graphics.detail)")
                    .koanText(.fine, .muted)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
            } header: {
                KoanSectionHeader("Graphics")
            } footer: {
                Text("How much kōan spends on looking like itself. Every step down removes something that costs while the music plays — the colour drifting behind the window first, since it costs the most.")
                    .koanText(.fine, .muted)
            }
        }
        .koanSheet()
    }
}

#if os(macOS)
extension DevicesSettings {
    /// Staying reachable with the window closed. The setting is written
    /// through the model, which would otherwise write its own copy back.
    private var background: some View {
        let residency = app.residency
        return Section {
            Toggle(
                "Keep running in the menu bar",
                isOn: Binding(
                    get: { model.settings.devicesKeepRunning },
                    set: { on in
                        model.edit { $0.devicesKeepRunning = on }
                        residency.keepRunning = on
                        // Turned off from the menu bar, with the window
                        // closed: closing Settings would otherwise quit kōan
                        // and leave it to reopen with no window.
                        if !on, !Residency.mainWindowShown {
                            openWindow(id: MainWindow.id)
                        }
                    })).koanToggle()
            Toggle(
                "Open at login",
                isOn: Binding(
                    get: { residency.opensAtLogin || residency.loginNeedsApproval },
                    set: { residency.setOpensAtLogin($0) })).koanToggle()
            if residency.loginNeedsApproval {
                Text("Allow kōan in System Settings ▸ General ▸ Login Items.")
                    .koanText(.meta, .bad)
            }
            if let error = residency.loginError {
                Text(error)
                    .koanText(.meta, .bad)
            }
        } header: {
            KoanSectionHeader("In the background")
        } footer: {
            Text("With its window closed, kōan stays in the menu bar, signed in to your server and listening on this network, so your other devices can see and control this Mac. A Mac cannot be woken from another device: once kōan is quit, it is out of reach until kōan is opened again.")
                .koanText(.fine, .muted)
        }
        .onAppear { residency.refreshLogin() }
    }
}
#endif

#if os(macOS)
/// Saves the settings window's frame under a name of its own and restores it
/// when the window opens, as `Window` scenes do and the `Settings` scene does
/// not.
private struct SettingsFrameAutosave: NSViewRepresentable {
    final class Probe: NSView {
        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            guard let window, window.frameAutosaveName.isEmpty else { return }
            window.setFrameUsingName("KoanSettings")
            window.setFrameAutosaveName("KoanSettings")
        }
    }

    func makeNSView(context: Context) -> Probe { Probe() }
    func updateNSView(_ view: Probe, context: Context) {}
}

/// The Settings panes, each a page of its own, for the evidence renderer: the
/// window's tabs show one at a time, and the panes are private to this file.
@MainActor
enum SettingsEvidence {
    static func pages(_ state: AppState) async -> [(name: String, size: CGSize, view: AnyView)] {
        let model = await SettingsModel(engine: state.library.engine, activity: state.activity, art: state.art)
        // What the Settings scene injects, so a pane renders as it does there.
        func page(_ view: some View) -> AnyView {
            AnyView(
                view
                    .environment(state)
                    .environment(state.player)
                    .environment(state.library)
                    .environment(state.activity)
                    .environment(state.art)
                    .environment(state.mirror)
                    .koanTheme(state.appearance)
            )
        }
        let size = CGSize(width: 820, height: 780)
        return [
            ("settings-library", size, page(LibrarySettings(model: model))),
            ("settings-server", size, page(RemoteSettings(model: model))),
            ("settings-playback", size, page(PlaybackSettings(model: model))),
            ("settings-eq", size, page(EqSettings())),
            ("settings-devices", size, page(DevicesSettings(model: model))),
            ("settings-appearance", size, page(AppearanceSettings())),
        ]
    }
}
#endif
