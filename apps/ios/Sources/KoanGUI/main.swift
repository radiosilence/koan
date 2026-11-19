import KoanFFI
import UIKit

// The kōan GUI app's entry. GPUI owns the application delegate and the run
// loop; what Apple's APIs keep in Swift is told about the engine once Rust
// has opened it, through the C functions in `platform/ios.rs`.

@_silgen_name("koan_gui_main")
private func koanGuiMain()

@_silgen_name("koan_gui_forward_delegate")
private func koanGuiForwardDelegate(_ delegate: UnsafeMutableRawPointer)

@_silgen_name("koan_gui_on_events")
private func koanGuiOnEvents(
    _ engine: @convention(c) () -> Void,
    _ alerts: @convention(c) () -> Void,
    _ refused: @convention(c) () -> Void
)

// Held for the life of the process: GPUI retains it too, once forwarded.
private let push = PushDelegate()
private var remoteActivity: RemoteActivityController?
private var networkWatch: NetworkWatch?
private var saveOnLeaving: SaveOnLeaving?
koanGuiForwardDelegate(Unmanaged.passUnretained(push).toOpaque())
koanGuiOnEvents(
    {
        Task { @MainActor in
            guard let engine = openedEngine() else { return }
            PushDelegate.engine = engine
            remoteActivity = RemoteActivityController(engine: engine)
            networkWatch = NetworkWatch(engine: engine)
            saveOnLeaving = SaveOnLeaving(engine: engine)
        }
    },
    { Task { await PushDelegate.requestAlerts() } },
    { PushDelegate.askToBeTapped() }
)
koanGuiMain()
