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
            Label(title, systemImage: symbol)
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
                    pane("Devices", "laptopcomputer.and.iphone") {
                        DevicesSettings(model: model)
                            .safeAreaInset(edge: .bottom) { StatusLine(model: model) }
                    }
                    Section {} footer: {
                        Text(AppVersion.text)
                            .font(.caption)
                            .foregroundStyle(.tertiary)
                            .frame(maxWidth: .infinity)
                    }
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
        Form {
            Section {
                if model.settings.libraryFolders.isEmpty {
                    Text("No folders yet — kōan has nothing to scan.")
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
                // Adding a folder starts a scan, so it waits for the one running.
                Button("Add Folder…") { choosingFolder = true }
                    .disabled(activity.conflicts(with: .localLibrary))
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
                // One pass over your files at a time. A sync or a download
                // clear is welcome to run alongside; another scan, a drop or a
                // file move would be reading and writing the same things.
                .disabled(activity.conflicts(with: .localLibrary))
            } header: {
                Text("Scan")
            } footer: {
                if activity.conflicts(with: .localLibrary) {
                    Text("Waiting for the task that is reading your files to finish.")
                        .font(.caption)
                        .foregroundStyle(.tertiary)
                }
            }

            Section {
                // Empties every table, so it waits for everything.
                Button("Clear Library Index…", role: .destructive) {
                    confirmingRebuild = true
                }
                .disabled(activity.conflicts(with: .wholeLibrary))
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
        Form {
            if model.settings.remoteSignedIn {
                Section("Signed in") {
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
                        Label(EngineMirror.signInRefusedDetail, systemImage: "exclamationmark.triangle")
                            .foregroundStyle(.orange)
                    }
                    HStack {
                        // Only the syncs wait on the database writer. Signing
                        // out is a config write, and greying it out while a
                        // sync runs strands you on a server you are trying to
                        // leave.
                        Button("Sync") { model.syncNow() }
                            .disabled(activity.conflicts(with: [.remoteTracks]))
                        #if !os(tvOS)
                        if mirror.offers(PasswordChange.extensionName) {
                            Button("Change Password…") { changingPassword = true }
                        }
                        #endif
                        Spacer()
                        Button("Sign Out", role: .destructive) { confirmingSignOut = true }
                    }
                    .rowButtons()
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
                ServerOffers()
            } else {
                Section {
                    // The prompt names the field: an iOS form shows only the
                    // prompt, so an example there leaves the field unlabelled.
                    TextField("Server URL", text: $url, prompt: Text("Server URL"))
                        .verbatimEntry(.url)
                        .accessibilityIdentifier("server-url")
                    TextField("Username", text: $username, prompt: Text("Username"))
                        .verbatimEntry()
                        .accessibilityIdentifier("username")
                    Picker("Sign in with", selection: $model.withApiKey) {
                        Text("Password").tag(false)
                        Text("API key").tag(true)
                    }
                    #if os(tvOS)
                    // Two choices side by side, rather than a page of their
                    // own to go into and come back from.
                    .pickerStyle(.segmented)
                    #endif
                    SecureField(
                        model.withApiKey ? "API key" : "Password",
                        text: $model.password,
                        prompt: Text(model.withApiKey ? "API key" : "Password")
                    )
                    .verbatimEntry()
                    .accessibilityIdentifier("secret")
                    HStack {
                        Button("Sign In") { model.signIn(url: url, username: username) }
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
                    Text("Subsonic or Navidrome")
                } footer: {
                    Text("Paste an invite here, or into Server URL, and kōan fills in the rest. The account is checked against the server, then saved to config.local.toml, readable only by you.")
                        .font(.caption)
                        .foregroundStyle(.tertiary)
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
                ))
            } footer: {
                Text("Shows only what is on this iPhone. It turns on by itself when your server cannot be reached, and off again when it can.")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
            }
            }
            #endif

            Section {
                Toggle("Keep the library in sync", isOn: model.binding(\.autoSync))
                if model.settings.autoSync {
                    Picker("Every", selection: model.binding(\.autoSyncIntervalMins)) {
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
                Text("Each sync asks the server only for what changed since the last.")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
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
                ))
            } header: {
                Text("Play queue")
            } footer: {
                Text("Saves this device's queue to your account on the server, where other apps can pick it up, and picks up a queue another app saved there when kōan starts. Moving music between kōan devices does not need it.")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
            }
            }

            Section("Downloads") {
                #if os(tvOS)
                // tvOS has no stepper.
                Picker("Parallel downloads", selection: Binding(
                    get: { Int(model.settings.downloadWorkers) },
                    set: { v in model.edit { $0.downloadWorkers = UInt32(v) } }
                )) {
                    ForEach(1...16, id: \.self) { Text("\($0)").tag($0) }
                }
                #else
                Stepper(
                    "Parallel downloads: \(model.settings.downloadWorkers)",
                    value: Binding(
                        get: { Int(model.settings.downloadWorkers) },
                        set: { v in model.edit { $0.downloadWorkers = UInt32(v) } }
                    ),
                    in: 1...16
                )
                #endif
                TextField("Cache limit, e.g. 50GB — blank for no limit", text: Binding(
                    get: { cacheLimit ?? model.settings.cacheLimit },
                    set: { cacheLimit = $0 }
                ))
                .verbatimEntry()
                .focused($cacheLimitFocused)
                .onSubmit(commitCacheLimit)
                .onChange(of: cacheLimitFocused) { _, focused in
                    if !focused { commitCacheLimit() }
                }
                LabeledContent("Using") {
                    HStack {
                        Text(Format.bytes(Int64(model.settings.cacheBytes)))
                        Button("Clear") { model.clearCache() }
                            .buttonStyle(.borderless)
                            .disabled(activity.conflicts(with: [.downloads]))
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
                Text("kōan asks the device to run at the source's sample rate, so nothing is resampled unless the device refuses.")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
            }

            Section {
                Toggle("Fade on pause", isOn: model.binding(\.fadeOnPause))
            } header: {
                Text("Transport")
            } footer: {
                Text("Pause and resume ramp the volume over a moment instead of cutting.")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
            }

            Section {
                Picker("ReplayGain", selection: model.binding(\.replaygain)) {
                    Text("Off").tag("off")
                    Text("Per track").tag("track")
                    Text("Per album").tag("album")
                }
                if model.settings.replaygain != "off" {
                    #if os(tvOS)
                    Picker("Pre-amp", selection: model.binding(\.preAmpDb)) {
                        ForEach(Array(stride(from: -15.0, through: 15.0, by: 0.5)), id: \.self) { db in
                            Text("\(db, specifier: "%.1f") dB").tag(db)
                        }
                    }
                    #else
                    Stepper(
                        "Pre-amp: \(model.settings.preAmpDb, specifier: "%.1f") dB",
                        value: model.binding(\.preAmpDb),
                        in: -15...15,
                        step: 0.5
                    )
                    #endif
                }
            } header: {
                Text("Loudness")
            } footer: {
                Text("Applies the gain written into the file's tags. Per album keeps the relative loudness within a record.")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
            }

            DspSettings()
        }
        .formStyle(.grouped)
    }
}

/// Correction for the output in use: a profile of bands, impulse responses or
/// both, imported from what other tools write.
private struct DspSettings: View {
    @Environment(AppState.self) private var app
    @State private var importing = false
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
                ))
                if let device = o.device, !o.profiles.isEmpty {
                    Picker("Profile for \(dsp.label(device))", selection: Binding(
                        get: { o.active ?? "" },
                        set: { dsp.use($0.isEmpty ? nil : $0) }
                    )) {
                        Text("None").tag("")
                        ForEach(o.profiles, id: \.name) { p in
                            Text(p.name).tag(p.name)
                        }
                    }
                    .disabled(!o.enabled)
                }
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
            #endif
            if let error = dsp.lastError {
                Text(error)
                    .font(.caption)
                    .foregroundStyle(.orange)
            }
        } header: {
            Text("EQ and convolution")
        } footer: {
            Text("AutoEQ and Equalizer APO text, impulse WAVs, Roon zips, Convolver .cfg and CamillaDSP configs. Importing into a profile of the same name adds to it. An output without a profile plays untouched.")
                .font(.caption)
                .foregroundStyle(.tertiary)
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
                        .font(.caption)
                        .foregroundStyle(.orange)
                } else {
                    Text(DspModel.describe(profile))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
            Spacer()
            if active {
                Image(systemName: "checkmark")
                    .foregroundStyle(.tint)
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
                    Button("Use for \(device)") { dsp.use(name) }
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
                    Button("Approve", action: approve)
                        .disabled(code.trimmingCharacters(in: .whitespaces).isEmpty)
                }
            } header: {
                Text("Pair a device")
            } footer: {
                Text("A television or another device without a keyboard shows a code while it waits. Enter it here to sign it in as you, with a key of its own that can be revoked on the server.")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
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
                    .foregroundStyle(.secondary)
            }
        } header: {
            Text("What the server offers")
        } footer: {
            Text("Asked when kōan signs in and whenever its link to the server reconnects. Features beyond Subsonic are used only where the server lists them.")
                .font(.caption)
                .foregroundStyle(.tertiary)
        }
    }

    private func extensionList(_ extensions: [ServerExtension]) -> some View {
        ForEach(extensions, id: \.name) { e in
            LabeledContent(e.name, value: e.versions.map { "v\($0)" }.joined(separator: ", "))
                .font(.callout)
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
    @State private var address = ""
    @State private var grantee = ""
    @State private var shareError: String?

    var body: some View {
        Form {
            Section {
                Toggle("Discoverable on this network", isOn: model.binding(\.devicesDiscoverable))
                Picker("Devices on this network", selection: model.binding(\.devicesNearbyControl)) {
                    Text("Full control").tag("full")
                    Text("Playback only").tag("playback")
                }
                if let port = mirror.connection?.listeningPort {
                    LabeledContent("Listening on port", value: String(port))
                }
                if mirror.connection?.localNetworkBlocked == true {
                    Label(LocalNetwork.blocked, systemImage: "wifi.exclamationmark")
                    .font(.callout)
                    .foregroundStyle(.orange)
                }
            } header: {
                Text("This device")
            } footer: {
                Text("Any kōan app on this network can then see what is playing here and control it, whoever is signed in there: with Full control, the output, preset and volume too, and move the music here or away; with Playback only, play and the queue. Neither reaches your library, playlists or history. Choose Playback only on a network you share with strangers. Your own devices reach each other through your server either way.")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
            }

            Section {
                ForEach(model.settings.devicesAddresses, id: \.self) { addr in
                    HStack {
                        Text(addr).font(.callout.monospaced())
                        Spacer()
                        Button("Remove", role: .destructive) {
                            model.edit { $0.devicesAddresses.removeAll { $0 == addr } }
                        }
                        .buttonStyle(.borderless)
                    }
                }
                HStack {
                    TextField("Address", text: $address, prompt: Text("host or host:port"))
                        .verbatimEntry(.url)
                        .onSubmit(add)
                    Button("Add", action: add)
                        .disabled(address.trimmingCharacters(in: .whitespaces).isEmpty)
                }
            } header: {
                Text("Devices by address")
            } footer: {
                Text("For networks that do not announce devices, such as a tailnet. The port is 5626 unless the other device says otherwise.")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
            }

            if mirror.connection?.sharing == true {
                Section {
                    ForEach(mirror.connection?.sharedWith ?? [], id: \.self) { account in
                        HStack {
                            Text(account)
                            Spacer()
                            Button("Stop sharing", role: .destructive) { share(account, allow: false) }
                                .buttonStyle(.borderless)
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
                        Button("Share") { share(grantee, allow: true) }
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
                        Text(error).font(.caption).foregroundStyle(.orange)
                    }
                } header: {
                    Text("Shared with other accounts")
                } footer: {
                    Text("From any network, they can see what this device is playing and control its playback as on your own network: play, pause, skip, the queue, the output, preset and volume, and moving the music here or to their own devices. Each does it as their own account: nothing of your library, playlists, favourites or history, and nothing of your settings beyond what is playing and where.")
                        .font(.caption)
                        .foregroundStyle(.tertiary)
                }
            }
        }
        .formStyle(.grouped)
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

    var body: some View {
        Form {
            Section {
                // Positioned by where a step sits in the list, not by its raw
                // value: the raw values are what is on disk and cannot be
                // reordered, and the cheapest step was added last. tvOS has
                // no slider; a picker in the same order stands in.
                #if os(tvOS)
                Picker("Level", selection: $graphics) {
                    ForEach(Graphics.allCases, id: \.self) { Text($0.label).tag($0) }
                }
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
                    Text(Graphics.allCases.first?.label ?? "").font(.caption)
                } maximumValueLabel: {
                    Text(Graphics.allCases.last?.label ?? "").font(.caption)
                }
                #endif
                Text("**\(graphics.label)** — \(graphics.detail)")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
            } header: {
                Text("Graphics")
            } footer: {
                Text("How much kōan spends on looking like itself. Every step down removes something that costs while the music plays — the colour drifting behind the window first, since it costs the most.")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
            }
        }
        .formStyle(.grouped)
    }
}
