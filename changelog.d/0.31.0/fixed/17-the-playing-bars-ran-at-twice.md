- **The playing bars ran at twice the rate of the numbers behind them.** The indicator's timeline was pinned to 1/60 while the analyser it reads only produces new levels at 1/30, so every other frame redrew data that had not moved. In this window a frame is not free — each one is a SwiftUI graph update, and each of those costs a whole-window Auto Layout pass. Both now read the same constant.

