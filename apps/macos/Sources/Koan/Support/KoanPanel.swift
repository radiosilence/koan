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
    /// The focus scope of the theme menu the view is in, if any.
    @Entry var koanMenuRows: Namespace.ID?
}

@MainActor
final class KoanPanelCoordinator: NSObject, NSWindowDelegate {
    var close: () -> Void = {}
    private var window: KoanPanelWindow?
    private var host: NSHostingController<AnyView>?
    private weak var anchor: NSView?
    private var monitors: [Any] = []
    private var observers: [NSObjectProtocol] = []

    func show(_ root: AnyView, from view: NSView) {
        // An anchor out of any window, such as a toolbar item moved into the
        // overflow menu, has nowhere to hang a panel from: it is closed, so
        // the control opens it again once it is back.
        guard let home = view.window else {
            close()
            return
        }
        // The window that holds the page, or the panel this one opens from. A
        // toolbar in full screen is a window of its own, which hides as the
        // pointer leaves it.
        var parent = home
        while !(parent is KoanPanelWindow), let up = parent.parent { parent = up }
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
        window.isReleasedWhenClosed = false
        window.isOpaque = false
        window.backgroundColor = .clear
        window.hasShadow = false
        window.appearance = parent.effectiveAppearance
        window.contentViewController = host
        window.delegate = self
        window.setAccessibilityRole(.popover)
        self.host = host
        self.window = window
        parent.addChildWindow(window, ordered: .above)
        place()
        window.makeKey()
        NSAccessibility.post(element: window, notification: .created)
        // The rows take the keyboard from the first of them; a menu's ticked
        // row takes it from there, as the scope's preferred focus.
        DispatchQueue.main.async { window.selectNextKeyView(nil) }
        watch(parent)
    }

    func hide() {
        monitors.forEach(NSEvent.removeMonitor)
        monitors = []
        observers.forEach(NotificationCenter.default.removeObserver)
        observers = []
        guard let window else { return }
        let wasKey = window.isKeyWindow
        let parent = window.parent
        parent?.removeChildWindow(window)
        window.orderOut(nil)
        self.window = nil
        host = nil
        // Focus goes back to the control that opened it, as a popover's does.
        if wasKey, let parent {
            parent.makeKey()
            if let anchor { NSAccessibility.post(element: anchor, notification: .focusedUIElementChanged) }
        }
    }

    nonisolated func windowDidResize(_ notification: Notification) {
        MainActor.assumeIsolated { place() }
    }

    /// Beside the anchor, on the side of it with more of the parent window,
    /// its leading edge on the anchor's and kept on the screen.
    private func place() {
        guard let window, let anchor, let parent = anchor.window else { return }
        let top = window.parent ?? parent
        let gap: CGFloat = 4
        let button = parent.convertToScreen(anchor.convert(anchor.bounds, to: nil))
        let size = window.frame.size
        // A toolbar in full screen is a strip of a window: the screen is the
        // room there.
        let room = parent === top ? parent.frame : (top.screen?.visibleFrame ?? top.frame)
        let above = button.midY < room.midY
        var origin = CGPoint(
            x: button.minX,
            y: above ? button.maxY + gap : button.minY - gap - size.height
        )
        if let screen = (top.screen ?? NSScreen.main)?.visibleFrame {
            origin.x = min(max(origin.x, screen.minX), screen.maxX - size.width)
            origin.y = min(max(origin.y, screen.minY), screen.maxY - size.height)
        }
        window.setFrameOrigin(origin)
    }

    private func watch(_ parent: NSWindow) {
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
        // Only the panel the key is typed in: a panel opened from this one
        // closes alone.
        let escape = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self, event.keyCode == 53, event.window === self.window else { return event }
            self.close()
            return nil
        }
        monitors = [outside, escape].compactMap(\.self)
        let center = NotificationCenter.default
        observers = [
            center.addObserver(forName: NSApplication.didResignActiveNotification, object: nil, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated { self?.close() }
            },
            // The anchor moves with the window's layout; the panel follows it.
            center.addObserver(forName: NSWindow.didResizeNotification, object: parent, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated { self?.place() }
            },
        ]
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
    @Namespace private var rows

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
                        .focusScope(rows)
                        .environment(\.koanMenuRows, rows)
                        // Up and down move between rows, as in a menu.
                        .onMoveCommand { direction in
                            switch direction {
                            case .down: NSApp.keyWindow?.selectNextKeyView(nil)
                            case .up: NSApp.keyWindow?.selectPreviousKeyView(nil)
                            default: break
                            }
                        }
                }
        } else {
            Menu(content: content, label: label) // theme: raw — the platform's look
        }
        #else
        Menu(content: content, label: label) // theme: raw — the platform's look
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
        Picker(title, selection: $selection) { // theme: raw — the platform's look
            ForEach(options, id: \.value) { Text($0.label).tag($0.value) }
        }
        .pickerStyle(.inline)
        .labelsHidden()
    }
}

/// One option of a theme menu, ticked when it is the one chosen.
struct KoanMenuChoice<Label: View>: View {
    let chosen: Bool
    let action: () -> Void
    @ViewBuilder let label: () -> Label

    init(chosen: Bool, action: @escaping () -> Void, @ViewBuilder label: @escaping () -> Label) {
        self.chosen = chosen
        self.action = action
        self.label = label
    }

    #if os(macOS)
    @Environment(\.koanMenuRows) private var rows
    #endif

    var body: some View {
        let button = Button(action: action) {
            HStack(spacing: KoanTheme.Space.m) {
                label()
                Spacer(minLength: 0)
                KoanIcon("checkmark").opacity(chosen ? 1 : 0)
            }
        }
        .accessibilityAddTraits(chosen ? .isSelected : [])
        #if os(macOS)
        if let rows {
            button.prefersDefaultFocus(chosen, in: rows)
        } else {
            button
        }
        #else
        button
        #endif
    }
}

extension KoanMenuChoice where Label == Text {
    init(_ title: String, chosen: Bool, action: @escaping () -> Void) {
        self.init(chosen: chosen, action: action) { Text(title) }
    }
}

/// A choice set out in a form: in the theme on the Mac, the options as rows
/// with a tick on the chosen one, in place of AppKit's round radio buttons;
/// the system's inline picker otherwise, which a phone draws as ticked rows.
struct KoanChoices<Value: Hashable, Row: View>: View {
    let title: String
    @Binding var selection: Value
    let values: [Value]
    @ViewBuilder let row: (Value) -> Row

    var body: some View {
        #if os(macOS)
        if KoanTheme.isOn {
            VStack(alignment: .leading, spacing: 0) {
                ForEach(values, id: \.self) { value in
                    KoanMenuChoice(chosen: value == selection) { selection = value } label: { row(value) }
                        .padding(.vertical, KoanTheme.Space.xs)
                        .koanButton(.card)
                }
            }
            .accessibilityElement(children: .contain)
            .accessibilityLabel(title)
        } else {
            picker
        }
        #else
        picker
        #endif
    }

    private var picker: some View {
        Picker(title, selection: $selection) { // theme: raw — the platform's look
            ForEach(values, id: \.self) { row($0).tag($0) }
        }
        .pickerStyle(.inline)
        .labelsHidden()
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

/// A real button, so the keyboard and VoiceOver reach it: focusable without
/// Full Keyboard Access, as a menu's items are, and chosen with Return or
/// Space. The row under the pointer or the keyboard takes `surface`.
private struct KoanMenuRowBody: View {
    let configuration: PrimitiveButtonStyleConfiguration
    @Environment(\.koanClosePanel) private var close
    @Environment(\.isEnabled) private var enabled
    @State private var hovering = false
    @FocusState private var focused: Bool

    var body: some View {
        Button(role: configuration.role, action: choose) {
            configuration.label
                .font(.koan(.control))
                .foregroundStyle(tone)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, KoanTheme.Space.m)
                .padding(.vertical, KoanTheme.Space.xs)
                .background((hovering || focused) && enabled ? Color.koanSurface : Color.clear)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .focusable(interactions: .activate)
        .focused($focused)
        .focusEffectDisabled()
        .onHover { hovering = $0 }
        .onKeyPress(.return) {
            guard enabled else { return .ignored }
            choose()
            return .handled
        }
    }

    private func choose() {
        configuration.trigger()
        close()
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
