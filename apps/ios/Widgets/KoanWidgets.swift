import ActivityKit
import AppIntents
import SwiftUI
import WidgetKit

/// The widget extension: what iOS draws for kōan outside the app. For now,
/// the Live Activity for another device this phone is controlling.
@main
struct KoanWidgets: WidgetBundle {
    var body: some Widget {
        RemoteActivityWidget()
    }
}

struct RemoteActivityWidget: Widget {
    var body: some WidgetConfiguration {
        ActivityConfiguration(for: RemoteActivity.self) { context in
            LockScreen(state: context.state, device: context.attributes.deviceId)
                .padding(16)
                .activityBackgroundTint(nil)
        } dynamicIsland: { context in
            let state = context.state
            let device = context.attributes.deviceId
            return DynamicIsland {
                DynamicIslandExpandedRegion(.leading) {
                    Label(state.device, systemImage: "laptopcomputer.and.iphone")
                        .font(.caption)
                        .lineLimit(1)
                }
                DynamicIslandExpandedRegion(.center) {
                    Titles(state: state)
                }
                DynamicIslandExpandedRegion(.bottom) {
                    VStack(spacing: 8) {
                        Progress(state: state)
                        Buttons(state: state, device: device)
                    }
                }
            } compactLeading: {
                Image(systemName: "laptopcomputer.and.iphone")
            } compactTrailing: {
                Image(systemName: state.playing ? "waveform" : "pause.fill")
            } minimal: {
                Image(systemName: state.playing ? "waveform" : "pause.fill")
            }
        }
    }
}

private struct LockScreen: View {
    let state: RemoteActivity.ContentState
    let device: String

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Label("Playing on \(state.device)", systemImage: "laptopcomputer.and.iphone")
                .font(.caption)
                .foregroundStyle(.secondary)
            HStack(alignment: .center, spacing: 12) {
                Titles(state: state)
                    .frame(maxWidth: .infinity, alignment: .leading)
                Buttons(state: state, device: device)
            }
            Progress(state: state)
        }
    }
}

private struct Titles: View {
    let state: RemoteActivity.ContentState

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(state.title ?? "Nothing playing")
                .font(.headline)
                .lineLimit(1)
            if let artist = state.artist {
                Text(artist)
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
        }
    }
}

/// Runs on from where the device last said it was: iOS draws the moving bar,
/// so an update is needed only when the device does something.
private struct Progress: View {
    let state: RemoteActivity.ContentState

    var body: some View {
        if state.durationMs > 0 {
            if state.playing, state.ends > .now {
                ProgressView(timerInterval: state.started...state.ends, countsDown: false) {
                    EmptyView()
                } currentValueLabel: {
                    EmptyView()
                }
            } else {
                ProgressView(value: Double(state.positionMs), total: Double(state.durationMs))
            }
        }
    }
}

private struct Buttons: View {
    let state: RemoteActivity.ContentState
    let device: String

    var body: some View {
        HStack(spacing: 18) {
            Button(intent: RemoteControlIntent(device: device, command: #"{"type":"previous"}"#)) {
                Image(systemName: "backward.fill")
            }
            Button(intent: RemoteControlIntent(
                device: device,
                command: state.playing ? #"{"type":"pause"}"# : #"{"type":"resume"}"#
            )) {
                Image(systemName: state.playing ? "pause.fill" : "play.fill")
                    .font(.title2)
            }
            Button(intent: RemoteControlIntent(device: device, command: #"{"type":"next"}"#)) {
                Image(systemName: "forward.fill")
            }
        }
        .buttonStyle(.plain)
        .font(.title3)
    }
}
