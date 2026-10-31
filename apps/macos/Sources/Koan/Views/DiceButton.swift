import SwiftUI

/// Shuffles a list's rows in place, each press one undo step, so pressing
/// again keeps dealing and undo walks back to where it started. Not shuffle
/// play, which leaves the rows alone.
struct DiceButton: View {
    /// Why it cannot shuffle now, shown as its help; nil when it can.
    let refusal: String?
    let help: String
    var labelled = false
    let shuffle: () -> Void

    var body: some View {
        // The reason it is off goes on a wrapper, which disabling the button
        // does not reach.
        HStack {
            Button(action: shuffle) {
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
}
