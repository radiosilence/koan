import KoanFFI
import SwiftUI
#if os(iOS)
import UIKit
#endif

/// Where music plays: this device, another of the account's, or any koan app
/// on the same network.
///
/// A row is a device to control. Picking one pauses this device and turns the
/// transport, the queue and what is playing into that device's, until another
/// is picked. Nothing moves until "Move here", which sends what the device
/// being controlled is playing to that row's device and controls it there.
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

            footer
                .padding(.horizontal, 14)
                .padding(.vertical, 10)
        }
        .frame(minWidth: 320)
    }

    private var thisDevice: some View {
        DeviceChoiceRow(
            icon: Self.icon(for: Self.platform),
            name: "This \(Self.deviceNoun)",
            detail: player.isControllingAnother ? "Paused while you control another device" : "Music plays here",
            selected: !player.isControllingAnother,
            canMove: player.isControllingAnother && player.canMoveMusic(to: nil),
            onSelect: { player.control(nil) },
            onMove: { player.moveMusic(to: nil) }
        )
    }

    @ViewBuilder private var footer: some View {
        if mirror.devices.isEmpty {
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
        return "No other devices on this network. Signed in to a kōan server, your devices find each other anywhere."
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

/// iOS keeps an app off the local network until the person allows it, and
/// says nothing otherwise: without this the picker would just be empty.
private struct LocalNetworkBlocked: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Label("kōan can't see this network", systemImage: "wifi.exclamationmark")
                .font(.callout.weight(.medium))
            Text("Allow Local Network for kōan in Settings → Privacy & Security to find devices here.")
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
    @State private var open = false
    /// Show the controlled device's name beside the icon.
    var labelled = true

    var body: some View {
        Button {
            open = true
        } label: {
            HStack(spacing: 5) {
                Image(systemName: "laptopcomputer.and.iphone")
                if labelled, let name = controlledName {
                    Text(name)
                        .lineLimit(1)
                }
            }
            .foregroundStyle(player.isControllingAnother ? Color.accentColor : .primary)
        }
        .buttonStyle(.plain)
        .help(controlledName.map { "Playing on \($0)" } ?? "Play on another device")
        .accessibilityLabel(controlledName.map { "Playing on \($0)" } ?? "Play on another device")
        #if os(macOS)
        .popover(isPresented: $open, arrowEdge: .top) { DevicePicker() }
        #else
        .sheet(isPresented: $open) {
            ScrollView { DevicePicker() }
                .presentationDetents([.medium, .large])
                .presentationDragIndicator(.visible)
        }
        #endif
    }

    private var controlledName: String? {
        guard player.isControllingAnother else { return nil }
        return player.controlled?.name ?? "another device"
    }
}
