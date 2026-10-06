- **The lyrics panel slides open.** Its state was `@AppStorage`, and a `UserDefaults` write publishes on its own after the transaction that caused it has gone — so the pane had no animation to expand with while everything around it was still moving. It is observable state that writes through to defaults now, so the change happens inside the transaction and the pane and the stage move together.

