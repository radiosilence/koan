#if os(macOS)
import AppKit
#endif
import SwiftUI

#if os(macOS)

/// The theme's popover on the Mac: a square child window of `bg` in a hairline
/// `rule`, hung from the control that opens it with no arrow. `NSPopover` is a
/// rounded pane of glass whose chrome nothing reaches, so the theme draws its
/// own. It opens on whichever side of the control has more of the window, so a
/// control in the transport opens upwards and one in the toolbar downwards.
/// A click outside it, Escape, or the app going to the background closes it,
/// as a transient popover does.
struct KoanPanelAnchor<Panel: View>: NSViewRepresentable {
    @Binding var isPresented: Bool
    let panel: () -> Panel

    func makeCoordinator() -> KoanPanelCoordinator { KoanPanelCoordinator() }

    func makeNSView(context: Context) -> NSView { NSView() }

    func updateNSView(_ view: NSView, context: Context) {
        let coordinator = context.coordinator
        let binding = $isPresented
        coordinator.close = { binding.wrappedValue = false }
        guard isPresented else {
            coordinator.hide()
            return
        }
        let environment = context.environment
        let close = coordinator.close
        let root = panel()
            .koanPopover()
            .overlay { Rectangle().strokeBorder(Color.koanRule, lineWidth: KoanTheme.hairline) }
            .environment(\.koanClosePanel, KoanClosePanel(close))
            .transformEnvironment(\.self) { $0 = environment }
        // Shown after this update: the anchor has no window until it is in one,
        // and a window cannot be ordered in from inside a view update.
        DispatchQueue.main.async {
            if binding.wrappedValue { coordinator.show(AnyView(root), from: view) }
        }
    }

    static func dismantleNSView(_ view: NSView, coordinator: KoanPanelCoordinator) {
        coordinator.hide()
    }
}

/// Closes the panel the view is in: what a row of a theme menu runs after its
/// action, as a menu closes on a choice.
struct KoanClosePanel {
    private let action: () -> Void
    init(_ action: @escaping () -> Void = {}) { self.action = action }
    func callAsFunction() { action() }
}

extension EnvironmentValues {
    @Entry var koanClosePanel = KoanClosePanel()
}

@MainActor
final class KoanPanelCoordinator: NSObject, NSWindowDelegate {
    var close: () -> Void = {}
    private var window: KoanPanelWindow?
    private var host: NSHostingController<AnyView>?
    private weak var anchor: NSView?
    private var monitors: [Any] = []
    private var resign: NSObjectProtocol?

    func show(_ root: AnyView, from view: NSView) {
        guard let parent = view.window else { return }
        anchor = view
        if let host {
            host.rootView = root
            return
        }
        let host = NSHostingController(rootView: root)
        host.sizingOptions = .preferredContentSize
        let window = KoanPanelWindow(
            contentRect: .zero,
            styleMask: [.borderless],
            backing: .buffered,
            defer: true
        )
        window.isOpaque = false
        window.backgroundColor = .clear
        window.hasShadow = false
        window.appearance = parent.effectiveAppearance
        window.contentViewController = host
        window.delegate = self
        self.host = host
        self.window = window
        parent.addChildWindow(window, ordered: .above)
        place()
        window.makeKey()
        watch()
    }

    func hide() {
        monitors.forEach(NSEvent.removeMonitor)
        monitors = []
        if let resign { NotificationCenter.default.removeObserver(resign) }
        resign = nil
        guard let window else { return }
        window.parent?.removeChildWindow(window)
        window.orderOut(nil)
        self.window = nil
        host = nil
    }

    nonisolated func windowDidResize(_ notification: Notification) {
        MainActor.assumeIsolated { place() }
    }

    /// Beside the anchor, on the side of it with more of the parent window,
    /// its leading edge on the anchor's and kept on the screen.
    private func place() {
        guard let window, let anchor, let parent = anchor.window else { return }
        let gap: CGFloat = 4
        let button = parent.convertToScreen(anchor.convert(anchor.bounds, to: nil))
        let size = window.frame.size
        let room = parent.frame
        let above = button.midY < room.midY
        var origin = CGPoint(
            x: button.minX,
            y: above ? button.maxY + gap : button.minY - gap - size.height
        )
        if let screen = (parent.screen ?? NSScreen.main)?.visibleFrame {
            origin.x = min(max(origin.x, screen.minX), screen.maxX - size.width)
            origin.y = min(max(origin.y, screen.minY), screen.maxY - size.height)
        }
        window.setFrameOrigin(origin)
    }

    private func watch() {
        let outside = NSEvent.addLocalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown, .otherMouseDown]) { [weak self] event in
            guard let self, let window = self.window else { return event }
            // In the panel, or in a panel opened from it.
            var inside = event.window
            while let candidate = inside, candidate !== window { inside = candidate.parent }
            if inside != nil { return event }
            // A click on the control that opened it closes it rather than
            // opening it again.
            if let anchor = self.anchor, event.window === anchor.window,
               anchor.bounds.contains(anchor.convert(event.locationInWindow, from: nil)) {
                self.close()
                return nil
            }
            self.close()
            return event
        }
        let escape = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self, event.keyCode == 53 else { return event }
            self.close()
            return nil
        }
        monitors = [outside, escape].compactMap(\.self)
        resign = NotificationCenter.default.addObserver(
            forName: NSApplication.didResignActiveNotification, object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated { self?.close() }
        }
    }
}

#endif

/// A menu of actions. In the theme on the Mac, a button that opens the theme's
/// panel with the menu's buttons as rows, each closing it once chosen: an
/// `NSMenu` is drawn in glass that nothing reaches. The system's menu
/// otherwise.
struct KoanMenu<Content: View, Label: View>: View {
    @ViewBuilder let content: () -> Content
    @ViewBuilder let label: () -> Label
    @State private var open = false

    var body: some View {
        #if os(macOS)
        if KoanTheme.isOn {
            Button { open = true } label: { label() }
                .koanPopover(isPresented: $open, arrowEdge: .top) {
                    VStack(alignment: .leading, spacing: 0) { content() }
                        .buttonStyle(KoanMenuRow())
                        .padding(.vertical, KoanTheme.Space.xs)
                        .frame(minWidth: 180, alignment: .leading)
                        .fixedSize()
                }
        } else {
            Menu(content: content, label: label)
        }
        #else
        Menu(content: content, label: label)
        #endif
    }
}

/// A choice inside a `KoanMenu`: in the theme on the Mac, the options as rows
/// with a tick on the chosen one; the system's inline picker otherwise.
struct KoanMenuChoices<Value: Hashable>: View {
    let title: String
    @Binding var selection: Value
    let options: [(label: String, value: Value)]

    var body: some View {
        #if os(macOS)
        if KoanTheme.isOn {
            ForEach(options, id: \.value) { option in
                KoanMenuChoice(option.label, chosen: option.value == selection) { selection = option.value }
            }
        } else {
            picker
        }
        #else
        picker
        #endif
    }

    private var picker: some View {
        Picker(title, selection: $selection) {
            ForEach(options, id: \.value) { Text($0.label).tag($0.value) }
        }
        .pickerStyle(.inline)
        .labelsHidden()
    }
}

/// One option of a theme menu, ticked when it is the one chosen.
struct KoanMenuChoice: View {
    let label: String
    let chosen: Bool
    let action: () -> Void

    init(_ label: String, chosen: Bool, action: @escaping () -> Void) {
        self.label = label
        self.chosen = chosen
        self.action = action
    }

    var body: some View {
        Button(action: action) {
            HStack(spacing: KoanTheme.Space.m) {
                Text(label)
                Spacer(minLength: 0)
                KoanIcon("checkmark").opacity(chosen ? 1 : 0)
            }
        }
        .accessibilityAddTraits(chosen ? .isSelected : [])
    }
}

#if os(macOS)
/// A row of a `KoanMenu`: the label in `control` type, `surface` under the
/// pointer, `bad` for a destructive action.
private struct KoanMenuRow: PrimitiveButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        KoanMenuRowBody(configuration: configuration)
    }
}

private struct KoanMenuRowBody: View {
    let configuration: PrimitiveButtonStyleConfiguration
    @Environment(\.koanClosePanel) private var close
    @Environment(\.isEnabled) private var enabled
    @State private var hovering = false

    var body: some View {
        configuration.label
            .font(.koan(.control))
            .foregroundStyle(tone)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, KoanTheme.Space.m)
            .padding(.vertical, KoanTheme.Space.xs)
            .background(hovering && enabled ? Color.koanSurface : Color.clear)
            .contentShape(Rectangle())
            .onHover { hovering = $0 }
            .onTapGesture {
                guard enabled else { return }
                configuration.trigger()
                close()
            }
            .accessibilityAddTraits(.isButton)
    }

    private var tone: Color {
        if !enabled { return .koanMuted }
        return configuration.role == .destructive ? .koanBad : .koanInk
    }
}

/// Borderless, and still able to take the keyboard: a filter field or a
/// stepper in the panel is typed into.
private final class KoanPanelWindow: NSPanel {
    override var canBecomeKey: Bool { true }
}
#endif
