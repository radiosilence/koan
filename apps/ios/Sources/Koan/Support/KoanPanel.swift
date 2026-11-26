import SwiftUI

/// A menu of actions: the system's menu, which a long press of the remote
/// opens.
struct KoanMenu<Content: View, Label: View>: View {
    @ViewBuilder let content: () -> Content
    @ViewBuilder let label: () -> Label

    var body: some View {
        Menu(content: content, label: label) // theme: raw — the platform's look
    }
}

/// A choice set out in a form: the system's inline picker, drawn as ticked
/// rows.
struct KoanChoices<Value: Hashable, Row: View>: View {
    let title: String
    @Binding var selection: Value
    let values: [Value]
    @ViewBuilder let row: (Value) -> Row

    var body: some View {
        Picker(title, selection: $selection) { // theme: raw — the platform's look
            ForEach(values, id: \.self) { row($0).tag($0) }
        }
        .pickerStyle(.inline)
        .labelsHidden()
    }
}
