import SwiftUI

/// Shuffles a list's rows in place, each press one undo step, so pressing
/// again keeps dealing and undo walks back to where it started. Not shuffle
/// play, which leaves the rows alone.
///
/// A phone has no ⌘Z, so there it offers Undo beside itself for as many
/// presses as it has made, until `reset` changes: a row added or removed is
/// another edit, which Undo would take back first.
struct DiceButton: View {
    /// Why it cannot shuffle now, shown as its help; nil when it can.
    let refusal: String?
    let help: String
    var labelled = false
    let reset: Int
    let shuffle: () -> Void
    let undo: () -> Void

    #if os(iOS)
    @State private var presses = 0
    #endif

    var body: some View {
        HStack(spacing: labelled ? 10 : 0) {
            #if os(iOS)
            if presses > 0 {
                Button {
                    presses -= 1
                    undo()
                } label: {
                    if labelled {
                        Label("Undo", koan: Icon.undo)
                    } else {
                        KoanIcon(Icon.undo).touchTarget()
                    }
                }
                .help("Undo the last shuffle")
            }
            #endif
            Button {
                #if os(iOS)
                presses += 1
                #endif
                shuffle()
            } label: {
                if labelled {
                    Label("Shuffle Order", koan: Icon.reshuffle)
                } else {
                    KoanIcon(Icon.reshuffle).touchTarget()
                }
            }
            .disabled(refusal != nil)
            .help(refusal ?? help)
        }
        #if os(iOS)
        .onChange(of: reset) { presses = 0 }
        #endif
    }
}
