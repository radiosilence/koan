import KoanFFI
import SwiftUI
#if os(iOS)
import UIKit
#endif

/// Where music plays: this device, another of the account's, any koan app on
/// the same network, or a UPnP amplifier or streamer.
///
/// A koan row is a device to control. Picking one pauses this device and turns
/// the transport, the queue and what is playing into that device's, until
/// another is picked. Nothing moves until "Move here", which sends what the
/// device being controlled is playing to that row's device and controls it
/// there.
///
/// A renderer row is different underneath: it becomes this device's output, as
/// a DAC would, so the transport and queue stay this device's own.
struct DevicePicker: View {
    @Environment(PlayerModel.self) private var player
    @Environment(EngineMirror.self) private var mirror

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("Play on")
                .font(.headline)
                .padding(.horizontal, 14)
                .padding(.top, 12)
                .padding(.bottom, 6)

            if mirror.connection?.localNetworkBlocked == true {
                LocalNetworkBlocked()
                    .padding(.horizontal, 14)
                    .padding(.bottom, 8)
            }

            thisDevice
            ForEach(mirror.devices, id: \.id) { device in
                DeviceRow(device: device)
            }
            ForEach(mirror.renderers, id: \.udn) { renderer in
                RendererRow(renderer: renderer)
            }
            if let output = player.renderer {
                RendererVolume(output: output)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 6)
            }

            footer
                .padding(.horizontal, 14)
                .padding(.vertical, 10)
        }
        .frame(minWidth: 320)
        .onAppear { player.searchRenderers() }
    }

    private var thisDevice: some View {
        DeviceChoiceRow(
            icon: Self.icon(for: Self.platform),
            name: "This \(Self.deviceNoun)",
            detail: thisDetail,
            selected: !player.isControllingAnother && player.renderer == nil,
            canMove: player.isControllingAnother && player.canMoveMusic(to: nil),
            onSelect: {
                if player.renderer != nil {
                    player.playOn(renderer: nil)
                } else {
                    player.control(nil)
                }
            },
            onMove: { player.moveMusic(to: nil) }
        )
    }

    private var thisDetail: String {
        if player.isControllingAnother { return "Paused while you control another device" }
        if let renderer = player.renderer { return "Playing through \(renderer.name)" }
        return "Music plays here"
    }

    @ViewBuilder private var footer: some View {
        if mirror.devices.isEmpty && mirror.renderers.isEmpty {
            Text(emptyExplanation)
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        } else {
            Text("Pick a device to control it. Move here sends what is playing to that device.")
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private var emptyExplanation: String {
        if mirror.connection?.devices == true {
            return "No other devices. Open kōan on another device signed in to this server, or on this network."
        }
        return "No other devices on this network. Devices signed in to one kōan server reach each other through it, on any network."
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

    var body: some View {
        let output = player.renderer?.udn == renderer.udn ? player.renderer : nil
        DeviceChoiceRow(
            icon: "hifispeaker",
            name: renderer.name,
            detail: detail(output),
            reach: "wifi",
            reachHelp: "UPnP, on this network",
            selected: output != nil,
            canMove: false,
            onSelect: { player.playOn(renderer: renderer.udn) },
            onMove: {}
        )
    }

    private func detail(_ output: RendererOutput?) -> String {
        if let problem = output?.problem { return problem }
        if output != nil { return "Playing from this \(DevicePicker.deviceNoun)" }
        let model = [renderer.manufacturer, renderer.model]
            .filter { !$0.isEmpty }
            .joined(separator: " ")
        return model.isEmpty ? "Amplifier or streamer" : model
    }
}

/// The renderer's own volume. koan sends it the original file, so ReplayGain
/// and fades stay out of the signal; saying so beats ignoring them silently.
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
            Text("\(output.name) plays the original files. ReplayGain and fades don't apply.")
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
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
    let canMove: Bool
    /// Found but not reached: shown with the reason, and not pickable unless
    /// it is already the one picked.
    var unreachable = false
    let onSelect: () -> Void
    let onMove: () -> Void

    var body: some View {
        HStack(spacing: 12) {
            Button(action: onSelect) {
                HStack(spacing: 12) {
                    Image(systemName: icon)
                        .font(.title3)
                        .frame(width: 28)
                        .foregroundStyle(selected ? Color.accentColor : .secondary)
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
                        Text(detail)
                            .font(.caption)
                            .foregroundStyle(unreachable ? AnyShapeStyle(.orange) : AnyShapeStyle(.secondary))
                            .lineLimit(2)
                    }
                    Spacer(minLength: 0)
                    if selected {
                        Image(systemName: "checkmark")
                            .font(.body.weight(.semibold))
                            .foregroundStyle(Color.accentColor)
                            .accessibilityLabel("Controlling")
                    }
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .disabled(unreachable && !selected)

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
        .background(selected ? Color.accentColor.opacity(0.08) : .clear)
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

    var body: some View {
        Button {
            open = true
        } label: {
            // Only the icon takes the tint: it follows the record's colour,
            // and a dark sleeve makes tinted text vanish against the bar.
            HStack(spacing: 5) {
                Image(systemName: "laptopcomputer.and.iphone")
                    .foregroundStyle(controlledName != nil ? Color.accentColor : .primary)
                if labelled, let name = controlledName {
                    Text(name)
                        .lineLimit(1)
                        .foregroundStyle(.primary)
                }
            }
        }
        .buttonStyle(.plain)
        .help(controlledName.map { "Playing on \($0)" } ?? "Play on another device")
        .accessibilityLabel(controlledName.map { "Playing on \($0)" } ?? "Play on another device")
        #if os(macOS)
        .popover(isPresented: $open, arrowEdge: .top) { DevicePicker() }
        #endif
    }

    private var controlledName: String? {
        if let renderer = player.renderer { return renderer.name }
        guard player.isControllingAnother else { return nil }
        return player.controlled?.name ?? "another device"
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
