import KoanFFI
import SwiftUI
#if os(iOS)
import UIKit
#endif

/// Where music plays, as two choices.
///
/// **Control** is which kōan the transport, the queue and Now Playing show and
/// command: this device, or another of the account's or the network's.
/// Picking another pauses this one and turns everything into that device's,
/// until another is picked. Nothing moves until "Move here", which sends what
/// the device being controlled is playing to that row's device.
///
/// **Output** is where the device in view plays: its own audio devices, or a
/// UPnP amplifier it can see. Picking one moves only the sound; the queue and
/// transport stay where they are. While another device is controlled, these
/// are that device's outputs, as it published them, and picking one switches
/// it there.
enum DevicePicker {
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
        #elseif os(tvOS)
        "tvos"
        #else
        "macos"
        #endif
    }

    @MainActor static var deviceNoun: String {
        #if os(iOS)
        UIDevice.current.userInterfaceIdiom == .pad ? "iPad" : "iPhone"
        #elseif os(tvOS)
        "Apple TV"
        #else
        "Mac"
        #endif
    }

    static func icon(for platform: String) -> String {
        switch platform {
        case "ios": "iphone"
        case "tvos": "appletv"
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
            detail: device.owner.map { "Shared by \($0) · \(detail)" } ?? detail,
            reach: device.nearby ? "wifi" : "cloud",
            reachHelp: device.nearby ? "On this network" : "Through your server",
            selected: player.controlled?.id == device.id,
            action: .control,
            canMove: player.canMoveMusic(to: device),
            // Out of reach with nothing able to wake it: a command would go nowhere.
            unreachable: device.problem != nil || (!device.awake && !device.wakeable),
            warning: device.waking != nil || device.wakeFailed != nil,
            onSelect: { player.control(device.id) },
            onMove: { player.moveMusic(to: device.id) }
        )
        // A context menu on the Mac, a long press on iOS: where each looks
        // for what can be done to a row beyond choosing it.
        .contextMenu {
            if !device.awake {
                Button("Forget", systemImage: "trash", role: .destructive) {
                    player.forget(device.id)
                }
            }
        }
    }

    private var detail: String {
        if let problem = device.problem {
            return problem
        }
        if let stage = device.waking {
            return switch stage {
            case "tv": "Waking… waking the Apple TV"
            case "network": "Waking… trying it on this network"
            case "push": "Waking… sent a wake through your server"
            default: "Waking… tap the notification on \(device.name)"
            }
        }
        if !device.awake && !device.asleep {
            return "Reconnecting…"
        }
        if !device.awake {
            let seen = Self.seen(device.lastSeen).map { " · seen \($0)" } ?? ""
            if let failed = device.wakeFailed {
                return "\(failed) Asleep\(seen)."
            }
            if device.wakeable {
                return "Asleep\(seen). Choosing it wakes it; music sent here arrives as a notification to tap."
            }
            // A Mac drops out when kōan is quit or the Mac is off, and nothing
            // reaches it until someone opens kōan there.
            return device.platform == "macos"
                ? "Not running\(seen). It can't be woken from here."
                : "Asleep\(seen)"
        }
        let playing = [device.title, device.artist].compactMap { $0 }.joined(separator: " — ")
        let library = device.sameLibrary ? "" : " · different library"
        switch device.state {
        case .playing: return "Playing \(playing)\(library)"
        case .paused: return "Paused · \(playing)\(library)"
        case .stopped: return "Idle\(library)"
        }
    }

    private static let ago: RelativeDateTimeFormatter = {
        let f = RelativeDateTimeFormatter()
        f.unitsStyle = .short
        return f
    }()

    /// "5 min. ago", from Unix seconds.
    private static func seen(_ at: Int64?) -> String? {
        guard let at else { return nil }
        return ago.localizedString(for: Date(timeIntervalSince1970: TimeInterval(at)), relativeTo: .now)
    }
}

/// Which kōan device is shown and commanded.
struct ControlPicker: View {
    @Environment(PlayerModel.self) private var player
    @Environment(EngineMirror.self) private var mirror

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("Control").koanCase()
                .font(.role(.body, system: .headline))
                .padding(.horizontal, 14)
                .padding(.top, 12)
                .padding(.bottom, 4)

            if mirror.connection?.localNetworkBlocked == true {
                LocalNetworkBlocked()
                    .padding(.horizontal, 14)
                    .padding(.bottom, 8)
            }

            DeviceChoiceRow(
                icon: DevicePicker.icon(for: DevicePicker.platform),
                name: "This \(DevicePicker.deviceNoun)",
                detail: player.isControllingAnother ? "Paused while you control another device" : "",
                selected: !player.isControllingAnother,
                action: .control,
                canMove: player.isControllingAnother && player.canMoveMusic(to: nil),
                onSelect: { player.control(nil) },
                onMove: { player.moveMusic(to: nil) }
            )
            ForEach(mirror.devices, id: \.id) { device in
                DeviceRow(device: device)
            }

            if mirror.devices.isEmpty {
                Text(emptyExplanation)
                    .font(.role(.fine, system: .caption))
                    .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 10)
            }
        }
        .frame(minWidth: 340)
        .padding(.bottom, 6)
    }

    private var emptyExplanation: String {
        if mirror.connection?.devices == true {
            return "No other kōan devices. Open kōan on another device signed in to this server, or on this network."
        }
        return "No other kōan devices on this network. Devices signed in to one kōan server reach each other through it, on any network."
    }
}

/// Where the device in view plays, with each output's preset.
struct OutputPicker: View {
    @Environment(PlayerModel.self) private var player
    @Environment(AppState.self) private var app

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 1) {
                Text("Output").koanCase()
                    .font(.role(.body, system: .headline))
                if let owner = player.outputs?.owner {
                    Text("On \(owner)")
                        .font(.role(.fine, system: .caption))
                        .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
                }
            }
            .padding(.horizontal, 14)
            .padding(.top, 12)
            .padding(.bottom, 4)

            if let outputs = player.outputs {
                rows(outputs)
            } else {
                Text("\(player.controlled?.name ?? "That device") has not said what it plays through. It may need updating.")
                    .font(.role(.fine, system: .caption))
                    .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 10)
            }
        }
        .frame(minWidth: 340)
        .padding(.bottom, 6)
        .onAppear {
            // The device in view lists its outputs again: this one, or the
            // one controlled, asked over the link to republish what moved.
            if player.isControllingAnother {
                player.refreshControlledOutputs()
            } else {
                player.searchRenderers()
                player.refreshDevices()
                app.dsp.reload()
            }
        }
    }

    @ViewBuilder private func rows(_ outputs: OutputsInfo) -> some View {
        // A phone's own output is its route, which the system chooses: the
        // row says which, and picking it brings the sound back from a renderer.
        // An Apple TV's is too.
        let platform = outputs.owner == nil ? DevicePicker.platform : player.controlled?.platform
        if platform == "ios" || platform == "tvos" {
            ForEach(outputs.devices, id: \.id) { device in
                OutputChoiceRow(output: device, outputs: outputs, choice: .default, icon: DevicePicker.icon(for: platform ?? "ios"))
            }
        } else {
            OutputChoiceRow(output: nil, outputs: outputs, choice: .default, icon: "speaker.wave.2")
            ForEach(outputs.devices, id: \.id) { device in
                OutputChoiceRow(output: device, outputs: outputs, choice: .device(name: device.id), icon: DevicePicker.icon(forOutput: device.kind))
            }
        }
        ForEach(outputs.renderers, id: \.id) { renderer in
            OutputChoiceRow(output: renderer, outputs: outputs, choice: .renderer(udn: renderer.id), icon: "hifispeaker")
        }
        if case .renderer(let udn) = outputs.current,
           let renderer = outputs.renderers.first(where: { $0.id == udn }) {
            RendererVolume(name: renderer.name, volume: outputs.volume, here: outputs.owner == nil)
                .padding(.horizontal, 14)
                .padding(.vertical, 6)
        }
    }
}

/// One output of the device in view: `output` nil for the system default.
private struct OutputChoiceRow: View {
    @Environment(PlayerModel.self) private var player
    @Environment(AppState.self) private var app
    let output: OutputInfo?
    let outputs: OutputsInfo
    let choice: OutputChoice
    let icon: String

    var body: some View {
        let selected = outputs.current == choice
        let presets = output.flatMap {
            Presets(output: $0, of: outputs, player: player)
        }
        DeviceChoiceRow(
            icon: icon,
            name: output?.name ?? "System Default",
            detail: detail(selected: selected, presets: presets),
            reach: output?.kind == "upnp" ? "wifi" : nil,
            reachHelp: output?.kind == "upnp" ? "UPnP, on this network" : nil,
            selected: selected,
            action: .output,
            canMove: false,
            warning: !selected && output?.busy == true,
            presets: presets,
            onSelect: { player.selectOutput(choice) },
            onMove: {}
        )
    }

    private func detail(selected: Bool, presets: Presets?) -> String {
        guard let output else { return "" }
        if selected, outputs.owner == nil, case .renderer = choice, let problem = player.renderer?.problem {
            return problem
        }
        if !selected && output.busy {
            return "In use by something else. Picking it takes over."
        }
        let kind = output.kind == "upnp" ? (output.detail.isEmpty ? "UPnP" : "\(output.detail) · UPnP") : nil
        return [kind, presets?.summary].compactMap { $0 }.joined(separator: " · ")
    }
}

/// The renderer's own volume, and what it is sent. Handed the original file,
/// it plays without ReplayGain or fades; saying so beats ignoring them
/// silently. On another device, only the volume.
private struct RendererVolume: View {
    @Environment(PlayerModel.self) private var player
    let name: String
    let volume: UInt8?
    /// Played to from this device, which knows what it sends.
    let here: Bool
    @State private var dragging: Double?

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            if let volume {
                HStack(spacing: 8) {
                    #if os(tvOS)
                    // No slider on tvOS: a step either way, as a remote's own
                    // volume buttons do.
                    Button("Quieter", systemImage: "speaker.fill") {
                        player.setOutputVolume(UInt8(max(0, Int(volume) - 5)))
                    }
                    Text("\(volume)")
                        .monospacedDigit()
                        .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
                    Button("Louder", systemImage: "speaker.wave.3.fill") {
                        player.setOutputVolume(UInt8(min(100, Int(volume) + 5)))
                    }
                    #else
                    Image(systemName: "speaker.fill")
                        .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
                    KoanSlider(
                        "Volume on \(name)",
                        value: Binding(
                            get: { dragging ?? Double(volume) },
                            set: { dragging = $0 }
                        ),
                        in: 0...100,
                        onEditingChanged: { editing in
                            if !editing, let value = dragging {
                                player.setOutputVolume(UInt8(value.rounded()))
                                dragging = nil
                            }
                        }
                    )
                    .labelsHidden()
                    Image(systemName: "speaker.wave.3.fill")
                        .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
                    #endif
                }
            }
            if here {
                Text(sent)
                    .font(.role(.fine, system: .caption))
                    .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private var sent: String {
        if let dsp = player.currentFormat?.dsp {
            return "\(name) is sent a stream processed through \u{201C}\(dsp.profile)\u{201D}. Fades don't apply."
        }
        return "\(name) plays the original files. ReplayGain and fades don't apply."
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
                .font(.role(.fine, system: .caption))
                .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
            VStack(alignment: .leading, spacing: 1) {
                Text(title)
                    .font(.role(.fine, system: .caption.weight(.semibold)))
                    .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
                    .textCase(.uppercase)
                Text(detail)
                    .font(.role(.fine, system: .caption2))
                    .foregroundStyle(KoanTheme.style(.muted, system: .tertiary))
            }
            Spacer(minLength: 0)
            if let move {
                Button("Move here", action: move)
                    .font(.role(.fine, system: .caption))
                    .koanButton(.standard, system: .bordered)
                    .controlSize(.small)
                    .help("Bring what the other device is playing back here")
            }
        }
        .padding(.horizontal, 14)
        .padding(.top, 10)
        .padding(.bottom, 4)
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
                        .font(.role(.titleSmall, system: .title3))
                        .frame(width: 28)
                        .foregroundStyle(selected ? AnyShapeStyle(.tint) : KoanTheme.style(.muted, system: .secondary))
                    VStack(alignment: .leading, spacing: 2) {
                        HStack(spacing: 5) {
                            Text(name)
                                .font(.role(.body, system: .body.weight(selected ? .semibold : .regular)))
                                .lineLimit(1)
                            if let reach {
                                Image(systemName: reach)
                                    .font(.role(.fine, system: .caption2))
                                    .foregroundStyle(KoanTheme.style(.muted, system: .tertiary))
                                    .help(reachHelp ?? "")
                                    .accessibilityLabel(reachHelp ?? "")
                            }
                        }
                        // A row with nothing to add is one line, centred.
                        if !detail.isEmpty {
                            Text(detail)
                                .font(.role(.fine, system: .caption))
                                .foregroundStyle(
                                    unreachable || warning ? KoanTheme.style(.bad, system: .orange) : KoanTheme.style(.muted, system: .secondary)
                                )
                                .lineLimit(2)
                        }
                    }
                    Spacer(minLength: 0)
                    if selected {
                        Image(systemName: "checkmark")
                            .font(.role(.body, system: .body.weight(.semibold)))
                            .foregroundStyle(.tint)
                            .accessibilityLabel(action.selectedLabel)
                    } else if !canMove {
                        Image(systemName: action.glyph)
                            .font(.role(.fine, system: .caption))
                            .foregroundStyle(KoanTheme.style(.muted, system: .tertiary))
                            .help(action.help)
                            .accessibilityHidden(true)
                    }
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .disabled(unreachable && !selected)

            if let presets {
                PresetMenu(presets: presets) {
                    Image(systemName: "slider.horizontal.3")
                        .font(.role(.fine, system: .caption))
                        .foregroundStyle(presets.flat ? KoanTheme.style(.muted, system: .tertiary) : KoanTheme.style(.muted, system: .secondary))
                }
                #if os(macOS)
                .menuStyle(.borderlessButton)
                #endif
                .menuIndicator(.hidden)
                .fixedSize()
                .help("Preset")
            }

            if canMove {
                Button("Move here", action: onMove)
                    .font(.role(.fine, system: .caption))
                    .koanButton(.standard, system: .bordered)
                    .controlSize(.small)
                    .help("Send what is playing to \(name), and control it there")
            }
        }
        // In the theme the highlight starts where the title does, the row's
        // content inset within it, so the picker has one left edge.
        .padding(.horizontal, KoanTheme.isOn ? KoanTheme.Space.s : 14)
        .padding(.vertical, 8)
        .background(selected ? AnyShapeStyle(.tint.opacity(0.12)) : AnyShapeStyle(.clear))
        .padding(.horizontal, KoanTheme.isOn ? 14 : 0)
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }
}

enum LocalNetwork {
    #if os(iOS) || os(tvOS)
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
                .font(.role(.control, system: .callout.weight(.medium)))
            Text("Allow Local Network for kōan in \(LocalNetwork.settings) → Privacy & Security to find devices here.")
                .font(.role(.fine, system: .caption))
                .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
                .fixedSize(horizontal: false, vertical: true)
            #if os(iOS)
            Button("Open Settings") {
                if let url = URL(string: UIApplication.openSettingsURLString) {
                    UIApplication.shared.open(url)
                }
            }
            .font(.role(.fine, system: .caption))
            .buttonStyle(.bordered)
            .controlSize(.small)
            #endif
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(KoanTheme.style(.bad, system: .orange).opacity(0.12), in: RoundedRectangle(cornerRadius: KoanTheme.radius(10)))
    }
}

/// The button that opens Control, and names the device controlled when it is
/// not this one.
struct ControlButton: View {
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
            HStack(spacing: 5) {
                Image(systemName: Action.control.glyph)
                    .font(iconSize.map { .system(size: $0) })
                    .foregroundStyle(player.isControllingAnother ? AnyShapeStyle(.tint) : KoanTheme.style(.ink, system: .primary))
                if labelled, let name = controlled {
                    Text(name)
                        .lineLimit(1)
                        .foregroundStyle(KoanTheme.style(.ink, system: .primary))
                }
            }
        }
        .buttonStyle(.plain)
        .help(help)
        .accessibilityLabel(help)
        #if os(macOS)
        .koanPopover(isPresented: $open, arrowEdge: .top) { ControlPicker() }
        #endif
    }

    private var controlled: String? {
        player.isControllingAnother ? player.controlled?.name ?? "another device" : nil
    }

    private var help: String {
        controlled.map { "Controlling \($0)" } ?? "Control another kōan"
    }
}

/// The button that opens Output: where the device in view plays, named when it
/// is not that device's default, with a dot while what is heard is processed.
struct OutputButton: View {
    @Environment(PlayerModel.self) private var player
    @Binding var open: Bool
    var labelled = true
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
                    .foregroundStyle(elsewhere != nil ? AnyShapeStyle(.tint) : KoanTheme.style(.ink, system: .primary))
                    .overlay(alignment: .topTrailing) {
                        if processing != nil {
                            Circle()
                                .fill(.tint)
                                .frame(width: 5, height: 5)
                                .offset(x: 3, y: -1)
                        }
                    }
                if labelled, let name = elsewhere {
                    Text(name)
                        .lineLimit(1)
                        .foregroundStyle(KoanTheme.style(.ink, system: .primary))
                }
            }
        }
        .controlButton()
        .help(help)
        .accessibilityLabel(help)
        #if os(macOS)
        .koanPopover(isPresented: $open, arrowEdge: .top) { OutputPicker() }
        #endif
    }

    private var processing: String? { player.currentFormat?.dsp?.profile }

    /// The output by name, when it is a renderer: a local device is left to
    /// the help text, where its name beside the button would be noise.
    private var elsewhere: String? {
        guard let outputs = player.outputs, case .renderer(let udn) = outputs.current else { return nil }
        return outputs.renderers.first { $0.id == udn }?.name ?? player.renderer?.name
    }

    /// "Controlling MacBook · playing through Arcam", or where this device
    /// plays.
    private var help: String {
        let current = player.outputName
        var parts: [String] = []
        if player.isControllingAnother {
            parts.append("Controlling \(player.controlled?.name ?? "another device")")
        }
        if let current {
            parts.append(parts.isEmpty ? "Playing through \(current)" : "playing through \(current)")
        }
        if let processing {
            parts.append("through \u{201C}\(processing)\u{201D}")
        }
        return parts.isEmpty ? "Output" : parts.joined(separator: " · ")
    }
}

extension PlayerModel {
    /// What the device in view plays through, by name: a renderer, a device
    /// chosen by name, or this device's own default. `None` for another
    /// device's default, which it does not name.
    var outputName: String? {
        guard let outputs else { return nil }
        switch outputs.current {
        case .renderer(let udn): return outputs.renderers.first { $0.id == udn }?.name
        case .device(let name): return name
        // A phone's one device is its route, named even before the engine
        // has reported a current device.
        case .default:
            guard outputs.owner == nil else { return nil }
            return currentDevice ?? (outputs.devices.count == 1 ? outputs.devices.first?.name : nil)
        }
    }
}

#if os(iOS) || os(tvOS)
extension View {
    /// The sheets the buttons open, attached to a view that outlives them.
    func controlSheet(isPresented: Binding<Bool>) -> some View {
        tray(isPresented: isPresented) { ControlPicker() }
    }

    func outputSheet(isPresented: Binding<Bool>) -> some View {
        tray(isPresented: isPresented) { OutputPicker() }
    }
}
#endif
