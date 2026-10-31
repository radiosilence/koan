import SwiftUI

/// Shuffles a list's rows in place, each press one undo step, so pressing
/// again keeps dealing and undo walks back to where it started. Not shuffle
/// play, which leaves the rows alone.
///
/// A phone has no ⌘Z, so there it offers Undo beside itself for as many
/// presses as it has made. Any other edit to the list, which Undo would take
/// back first, puts it away: `version` moves with every edit, and each move
/// not owed to a press here or its Undo ends the run.
struct DiceButton: View {
    /// Why it cannot shuffle now, shown as its help; nil when it can.
    let refusal: String?
    let help: String
    var labelled = false
    let version: Int
    let shuffle: () -> Void
    let undo: () -> Void

    #if os(iOS)
    @State private var presses = 0
    /// What each edit sent from here and not yet seen in `version` will do to
    /// `presses` when it lands: a press counts once the list has moved.
    @State private var owed: [Int] = []
    #endif

    var body: some View {
        HStack(spacing: labelled ? 10 : 0) {
            #if os(iOS)
            if presses > 0 {
                Button {
                    owed.append(-1)
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
            // The reason it is off goes on a wrapper: a disabled control
            // shows no tooltip.
            HStack {
                Button {
                    #if os(iOS)
                    owed.append(1)
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
            }
            .help(refusal ?? help)
        }
        #if os(iOS)
        .onChange(of: version) {
            if owed.isEmpty {
                presses = 0
            } else {
                presses = max(0, presses + owed.removeFirst())
            }
        }
        #endif
    }
}
