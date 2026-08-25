import SwiftUI

extension EnvironmentValues {
    /// Whether the page this view belongs to is the one on screen. False only
    /// for the queue while you are somewhere else — see `StageView`. Anything
    /// that animates or subscribes to keep itself current reads it.
    @Entry var onStage = true
}
