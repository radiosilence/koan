import SwiftUI

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
        Menu(content: content, label: label) // theme: raw — the platform's look
    }
}

/// A choice inside a `KoanMenu`: in the theme on the Mac, the options as rows
/// with a tick on the chosen one; the system's inline picker otherwise.
struct KoanMenuChoices<Value: Hashable>: View {
    let title: String
    @Binding var selection: Value
    let options: [(label: String, value: Value)]

    var body: some View {
        picker
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

    var body: some View {
        let button = Button(action: action) {
            HStack(spacing: KoanTheme.Space.m) {
                label()
                Spacer(minLength: 0)
                KoanIcon(Icon.check).opacity(chosen ? 1 : 0)
            }
        }
        .accessibilityAddTraits(chosen ? .isSelected : [])
        button
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
        picker
    }

    private var picker: some View {
        Picker(title, selection: $selection) { // theme: raw — the platform's look
            ForEach(values, id: \.self) { row($0).tag($0) }
        }
        .pickerStyle(.inline)
        .labelsHidden()
    }
}

