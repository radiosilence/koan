import KoanFFI
import SwiftUI
#if os(iOS)
import UIKit
#endif

/// Where music plays, in one place, in two kinds.
///
/// **Outputs** are where this device's own music comes out: its speakers, a
/// DAC, a UPnP amplifier. AirPlay is the system's: a speaker chosen there
/// appears here as the AirPlay output, since apps cannot pick one themselves. Picking one keeps the queue and transport
/// here and moves only the sound.
///
/// **Other kōan devices** are controlled: picking one pauses this device and
/// turns the transport, the queue and what is playing into that device's,
/// until another is picked. Nothing moves until "Move here", which sends what
/// the device being controlled is playing to that row's device.
///
/// Each row carries the glyph of what picking it does, so the two kinds read
/// apart without being explained.
struct DevicePicker: View {
    @Environment(PlayerModel.self) private var player
    @Environment(EngineMirror.self) private var mirror
    @Environment(AppState.self) private var app

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("Play on")
                .font(.headline)
                .padding(.horizontal, 14)
                .padding(.top, 12)
                .padding(.bottom, 4)

            if mirror.connection?.localNetworkBlocked == true {
                LocalNetworkBlocked()
                    .padding(.horizontal, 14)
                    .padding(.bottom, 8)
            }

            outputs
            if !mirror.devices.isEmpty {
                SectionHeading(
                    title: "Control another kōan",
                    glyph: Action.control.glyph,
                    detail: "Shows and commands that device's own queue"
                )
                ForEach(mirror.devices, id: \.id) { device in
                    DeviceRow(device: device)
                }
            }

            footer
                .padding(.horizontal, 14)
                .padding(.vertical, 10)
        }
        .frame(minWidth: 340)
        .onAppear {
            player.searchRenderers()
            player.refreshDevices()
            app.dsp.reload()
        }
    }

    @ViewBuilder private var outputs: some View {
        SectionHeading(
            title: "Play from this \(Self.deviceNoun)",
            glyph: Action.output.glyph,
            detail: player.isControllingAnother
                ? "Paused while you control another device"
                : "The queue stays here; the sound goes there",
            move: player.isControllingAnother && player.canMoveMusic(to: nil)
                ? { player.moveMusic(to: nil) } : nil
        )
        #if os(macOS)
        OutputRow(
            icon: "speaker.wave.2",
            name: "System Default",
            detail: nil,
            selected: player.isPlayingHere(.system(nil)),
            onSelect: { player.playHere(.system(nil)) }
        )
        ForEach(player.devices, id: \.name) { device in
            let presets = Presets(dsp: app.dsp, device: device.name, none: "Off")
            OutputRow(
                icon: Self.icon(forOutput: device.kind),
                name: device.name,
                detail: presets?.summary,
                selected: player.isPlayingHere(.system(device.name)),
                presets: presets,
                onSelect: { player.playHere(.system(device.name)) }
            )
        }
        #else
        OutputRow(
            icon: Self.icon(for: Self.platform),
            name: "This \(Self.deviceNoun)",
            detail: nil,
            selected: player.isPlayingHere(.system(nil)),
            onSelect: { player.playHere(.system(nil)) }
        )
        #endif
        ForEach(mirror.renderers, id: \.udn) { renderer in
            RendererRow(
                renderer: renderer,
                presets: Presets(dsp: app.dsp, device: renderer.udn, none: "Original file")
            )
        }
        if let output = player.renderer, !player.isControllingAnother {
            RendererVolume(output: output)
                .padding(.horizontal, 14)
                .padding(.vertical, 6)
        }
    }

    @ViewBuilder private var footer: some View {
        if mirror.devices.isEmpty {
            Text(emptyExplanation)
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private var emptyExplanation: String {
        if mirror.connection?.devices == true {
            return "No other kōan devices. Open kōan on another device signed in to this server, or on this network."
        }
        return "No other kōan devices on this network. Devices signed in to one kōan server reach each other through it, on any network."
    }

    /// An output's icon by how it is connected.
    static func icon(forOutput kind: String) -> String {
        switch kind {
        case "builtin": "laptopcomputer"
        case "usb": "cable.connector"
        case "bluetooth": "headphones"
        case "airplay": "airplayaudio"
        case "display": "display"
        default: "hifispeaker"
        }
    }

    static var platform: String {
        #if os(iOS)
        "ios"
        #else
        "macos"
        #endif
    }

    static var deviceNoun: String {
        #if os(iOS)
        UIDevice.current.userInterfaceIdiom == .pad ? "iPad" : "iPhone"
        #else
        "Mac"
        #endif
    }

    static func icon(for platform: String) -> String {
        switch platform {
        case "ios": "iphone"
        case "macos": "laptopcomputer"
        default: "desktopcomputer"
        }
    }
}

private struct DeviceRow: View {
    @Environment(PlayerModel.self) private var player
    let device: DeviceInfo

    var body: some View {
        DeviceChoiceRow(
            icon: DevicePicker.icon(for: device.platform),
            name: device.name,
            detail: detail,
            reach: device.nearby ? "wifi" : "cloud",
            reachHelp: device.nearby ? "On this network" : "Through your server",
            selected: player.controlled?.id == device.id,
            action: .control,
            canMove: player.canMoveMusic(to: device),
            unreachable: device.problem != nil,
            onSelect: { player.control(device.id) },
            onMove: { player.moveMusic(to: device.id) }
        )
    }

    private var detail: String {
        if let problem = device.problem {
            return problem
        }
        if !device.awake {
            return "Asleep. Music sent here arrives as a notification to tap."
        }
        let playing = [device.title, device.artist].compactMap { $0 }.joined(separator: " — ")
        let library = device.sameLibrary ? "" : " · different library"
        switch device.state {
        case .playing: return "Playing \(playing)\(library)"
        case .paused: return "Paused · \(playing)\(library)"
        case .stopped: return "Idle\(library)"
        }
    }
}

/// A UPnP renderer: tap to play this device's music through it.
private struct RendererRow: View {
    @Environment(PlayerModel.self) private var player
    let renderer: RendererInfo
    let presets: Presets?

    var body: some View {
        let output = player.renderer?.udn == renderer.udn ? player.renderer : nil
        DeviceChoiceRow(
            icon: "hifispeaker",
            name: renderer.name,
            detail: detail(output),
            reach: "wifi",
            reachHelp: "UPnP, on this network",
            selected: player.isPlayingHere(.renderer(renderer.udn)),
            action: .output,
            canMove: false,
            warning: output == nil && renderer.busy,
            presets: presets,
            onSelect: { player.playHere(.renderer(renderer.udn)) },
            onMove: {}
        )
    }

    private func detail(_ output: RendererOutput?) -> String {
        if let problem = output?.problem { return problem }
        let model = [renderer.manufacturer, renderer.model]
            .filter { !$0.isEmpty }
            .joined(separator: " ")
        let kind = model.isEmpty ? "UPnP" : "\(model) · UPnP"
        if output == nil && renderer.busy {
            return "In use by something else. Picking it takes over."
        }
        return [kind, presets?.summary].compactMap { $0 }.joined(separator: " · ")
    }
}

/// The renderer's own volume, and what it is sent. Handed the original file,
/// it plays without ReplayGain or fades; saying so beats ignoring them
/// silently.
private struct RendererVolume: View {
    @Environment(PlayerModel.self) private var player
    let output: RendererOutput
    @State private var dragging: Double?

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            if let volume = output.volume {
                HStack(spacing: 8) {
                    Image(systemName: "speaker.fill")
                        .foregroundStyle(.secondary)
                    Slider(
                        value: Binding(
                            get: { dragging ?? Double(volume) },
                            set: { dragging = $0 }
                        ),
                        in: 0...100,
                        onEditingChanged: { editing in
                            if !editing, let value = dragging {
                                player.setRendererVolume(UInt8(value.rounded()))
                                dragging = nil
                            }
                        }
                    )
                    .accessibilityLabel("Volume on \(output.name)")
                    Image(systemName: "speaker.wave.3.fill")
                        .foregroundStyle(.secondary)
                }
            }
            Text(sent)
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}

extension RendererVolume {
    private var sent: String {
        if let dsp = player.currentFormat?.dsp {
            return "\(output.name) is sent a stream processed through \u{201C}\(dsp.profile)\u{201D}. Fades don't apply."
        }
        return "\(output.name) plays the original files. ReplayGain and fades don't apply."
    }
}

/// The DSP profiles a device can play through, and the one it does. Nil when
/// there are no profiles to choose from, so rows without a choice say
/// nothing.
struct Presets {
    let current: String?
    let profiles: [String]
    /// What a device with no profile is said to play: "Off" for one of this
    /// device's own, "Original file" for a renderer.
    let none: String
    let enabled: Bool
    let choose: (String?) -> Void

    @MainActor
    init?(dsp: DspModel, device: String, none: String) {
        guard let overview = dsp.overview, !overview.profiles.isEmpty else { return nil }
        current = dsp.profile(for: device)
        profiles = overview.profiles.map(\.name)
        self.none = none
        enabled = overview.enabled
        choose = { dsp.assign($0, to: device) }
    }

    var summary: String {
        guard let current else { return none }
        return enabled ? current : "\(current), processing off"
    }
}

/// The preset submenu at a row's end.
private struct PresetMenu: View {
    let presets: Presets

    var body: some View {
        Menu {
            Picker("Preset", selection: Binding(
                get: { presets.current ?? "" },
                set: { presets.choose($0.isEmpty ? nil : $0) }
            )) {
                Text(presets.none).tag("")
                Divider()
                ForEach(presets.profiles, id: \.self) { Text($0).tag($0) }
            }
            .pickerStyle(.inline)
        } label: {
            Image(systemName: "slider.horizontal.3")
                .font(.caption)
                .foregroundStyle(presets.current == nil ? AnyShapeStyle(.tertiary) : AnyShapeStyle(.secondary))
        }
        #if os(macOS)
        .menuStyle(.borderlessButton)
        #endif
        .menuIndicator(.hidden)
        .fixedSize()
        .help("Preset")
        .accessibilityLabel("Preset: \(presets.summary)")
    }
}

/// What picking a row does.
enum Action {
    /// This device's music comes out there.
    case output
    /// This device shows and commands that one.
    case control

    var glyph: String {
        switch self {
        case .output: "speaker.wave.2"
        case .control: "av.remote"
        }
    }

    var help: String {
        switch self {
        case .output: "Play this device's music through it"
        case .control: "Control that device and its own queue"
        }
    }

    var selectedLabel: String {
        switch self {
        case .output: "Playing here"
        case .control: "Controlling"
        }
    }
}

/// A section's title, the glyph its rows carry, and what picking one does.
private struct SectionHeading: View {
    let title: String
    let glyph: String
    let detail: String
    /// "Move here", when there is music elsewhere to bring to this section.
    var move: (() -> Void)?

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            Image(systemName: glyph)
                .font(.caption)
                .foregroundStyle(.secondary)
            VStack(alignment: .leading, spacing: 1) {
                Text(title)
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(.secondary)
                    .textCase(.uppercase)
                Text(detail)
                    .font(.caption2)
                    .foregroundStyle(.tertiary)
            }
            Spacer(minLength: 0)
            if let move {
                Button("Move here", action: move)
                    .font(.caption)
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                    .help("Bring what the other device is playing back here")
            }
        }
        .padding(.horizontal, 14)
        .padding(.top, 10)
        .padding(.bottom, 4)
    }
}

/// One of this device's own outputs.
private struct OutputRow: View {
    let icon: String
    let name: String
    let detail: String?
    let selected: Bool
    var presets: Presets?
    let onSelect: () -> Void

    var body: some View {
        DeviceChoiceRow(
            icon: icon,
            name: name,
            detail: detail ?? "",
            selected: selected,
            action: .output,
            canMove: false,
            presets: presets,
            onSelect: onSelect,
            onMove: {}
        )
    }
}


/// One device: tap to control it, and a button to move the music there.
private struct DeviceChoiceRow: View {
    let icon: String
    let name: String
    let detail: String
    var reach: String?
    var reachHelp: String?
    let selected: Bool
    /// What picking it does, drawn at the row's end.
    let action: Action
    let canMove: Bool
    /// Found but not reached: shown with the reason, and not pickable unless
    /// it is already the one picked.
    var unreachable = false
    /// Pickable, with something worth reading first.
    var warning = false
    var presets: Presets?
    let onSelect: () -> Void
    let onMove: () -> Void

    var body: some View {
        HStack(spacing: 12) {
            Button(action: onSelect) {
                HStack(spacing: 12) {
                    Image(systemName: icon)
                        .font(.title3)
                        .frame(width: 28)
                        .foregroundStyle(selected ? AnyShapeStyle(.tint) : AnyShapeStyle(.secondary))
                    VStack(alignment: .leading, spacing: 2) {
                        HStack(spacing: 5) {
                            Text(name)
                                .font(.body.weight(selected ? .semibold : .regular))
                                .lineLimit(1)
                            if let reach {
                                Image(systemName: reach)
                                    .font(.caption2)
                                    .foregroundStyle(.tertiary)
                                    .help(reachHelp ?? "")
                                    .accessibilityLabel(reachHelp ?? "")
                            }
                        }
                        // A row with nothing to add is one line, centred.
                        if !detail.isEmpty {
                            Text(detail)
                                .font(.caption)
                                .foregroundStyle(
                                    unreachable || warning ? AnyShapeStyle(.orange) : AnyShapeStyle(.secondary)
                                )
                                .lineLimit(2)
                        }
                    }
                    Spacer(minLength: 0)
                    if selected {
                        Image(systemName: "checkmark")
                            .font(.body.weight(.semibold))
                            .foregroundStyle(.tint)
                            .accessibilityLabel(action.selectedLabel)
                    } else if !canMove {
                        Image(systemName: action.glyph)
                            .font(.caption)
                            .foregroundStyle(.tertiary)
                            .help(action.help)
                            .accessibilityHidden(true)
                    }
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .disabled(unreachable && !selected)

            if let presets {
                PresetMenu(presets: presets)
            }

            if canMove {
                Button("Move here", action: onMove)
                    .font(.caption)
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                    .help("Send what is playing to \(name), and control it there")
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 8)
        .background(selected ? AnyShapeStyle(.tint.opacity(0.12)) : AnyShapeStyle(.clear))
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }
}

enum LocalNetwork {
    #if os(iOS)
    static let settings = "Settings"
    #else
    static let settings = "System Settings"
    #endif
    static let blocked = "Blocked. Allow Local Network for kōan in \(settings) → Privacy & Security."
}

/// The system keeps an app off the local network until the person allows it,
/// and says nothing otherwise: without this the picker would just be empty.
private struct LocalNetworkBlocked: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Label("kōan can't see this network", systemImage: "wifi.exclamationmark")
                .font(.callout.weight(.medium))
            Text("Allow Local Network for kōan in \(LocalNetwork.settings) → Privacy & Security to find devices here.")
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            #if os(iOS)
            Button("Open Settings") {
                if let url = URL(string: UIApplication.openSettingsURLString) {
                    UIApplication.shared.open(url)
                }
            }
            .font(.caption)
            .buttonStyle(.bordered)
            .controlSize(.small)
            #endif
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.orange.opacity(0.12), in: RoundedRectangle(cornerRadius: 10))
    }
}

/// The button that opens the picker, and says which device is being
/// controlled when it is not this one.
struct DevicePickerButton: View {
    @Environment(PlayerModel.self) private var player
    /// Held by the host. On iOS the sheet hangs off a view that outlives the
    /// button: the tab bar accessory is rebuilt while a sheet is up on iPad,
    /// and a sheet bound to the button's own state goes with it.
    @Binding var open: Bool
    /// Show the controlled device's name beside the icon.
    var labelled = true
    /// The icon's size, where the host's font would draw it too small.
    var iconSize: CGFloat?

    var body: some View {
        Button {
            open = true
        } label: {
            // The icon says where the music is going and takes the tint; the
            // name stays primary, since a dark sleeve's tint vanishes as text.
            HStack(spacing: 5) {
                Image(systemName: "hifispeaker")
                    .font(iconSize.map { .system(size: $0) })
                    .foregroundStyle(target.name != nil ? AnyShapeStyle(.tint) : AnyShapeStyle(.primary))
                    // What is heard is processed: the badge says how.
                    .overlay(alignment: .topTrailing) {
                        if processing != nil {
                            Circle()
                                .fill(.tint)
                                .frame(width: 5, height: 5)
                                .offset(x: 3, y: -1)
                        }
                    }
                if labelled, let name = target.name {
                    Text(name)
                        .lineLimit(1)
                        .foregroundStyle(.primary)
                }
            }
        }
        .buttonStyle(.plain)
        .help(help)
        .accessibilityLabel(help)
        #if os(macOS)
        .popover(isPresented: $open, arrowEdge: .top) { DevicePicker() }
        #endif
    }

    private var processing: String? { player.currentFormat?.dsp?.profile }

    private var help: String {
        guard let processing else { return target.help }
        return "\(target.help), through \u{201C}\(processing)\u{201D}"
    }

    /// Where the music is going, when it is not this device's default
    /// output: another kōan being controlled, or a renderer. A local device is
    /// left to the help text; its name beside the button would be noise.
    private var target: (name: String?, help: String) {
        if player.isControllingAnother {
            let name = player.controlled?.name ?? "another device"
            return (name, "Controlling \(name)")
        }
        if let renderer = player.renderer {
            return (renderer.name, "Playing through \(renderer.name)")
        }
        if let name = player.currentDevice {
            return (nil, "Playing through \(name)")
        }
        return (nil, "Play on")
    }
}

#if os(iOS)
extension View {
    /// The sheet a `DevicePickerButton` opens, attached to a view that
    /// outlives the button.
    func devicePickerSheet(isPresented: Binding<Bool>) -> some View {
        sheet(isPresented: isPresented) {
            ScrollView { DevicePicker() }
                .presentationDetents([.medium, .large])
                .presentationDragIndicator(.visible)
        }
    }
}
#endif
