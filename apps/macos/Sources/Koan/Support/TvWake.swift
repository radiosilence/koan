import KoanFFI
import SwiftUI

/// Setting an Apple TV up to be woken from the account's devices.
///
/// The TV is paired once, the way the Remote app pairs: it shows a four-digit
/// code and this device types it. The server keeps the pairing for the
/// account, and from then on choosing the TV on any of the account's devices
/// on its network turns it on and opens kōan there, push or no push. The TV
/// hangs up about half a minute after asking, so the code is asked for only
/// once it is on screen.
///
/// Offered from the TV's menu in Play on, and straight after the TV is signed
/// in from here, once it links.
struct TvWakeTarget: Identifiable, Equatable {
    let id: String
    let name: String
}

/// A device signed in from here, by the id it links with, and when: an Apple
/// TV among them that links soon after is offered waking.
struct SignedInHere: Equatable {
    let id: String
    let at: Date
}

/// The offer after a sign-in, and the sheet that pairs.
struct TvWakePrompts: ViewModifier {
    let state: AppState

    /// The TV just signed in from here, once it has linked and if it is not
    /// paired already. A sign-in some minutes old is no longer what the
    /// person is doing.
    private var arrived: TvWakeTarget? {
        guard let signed = state.ui.signedInHere, signed.at.timeIntervalSinceNow > -300 else { return nil }
        return state.mirror.devices
            .first { $0.tvPairable && !$0.tvPaired && $0.awake && $0.id == signed.id }
            .map { TvWakeTarget(id: $0.id, name: $0.name) }
    }

    func body(content: Content) -> some View {
        content
            .onChange(of: arrived) { _, tv in
                guard let tv else { return }
                state.ui.signedInHere = nil
                state.ui.tvWakeOffer = tv
            }
            .alert(
                "Wake \(state.ui.tvWakeOffer?.name ?? "the Apple TV") from here?",
                isPresented: Binding(
                    get: { state.ui.tvWakeOffer != nil },
                    set: { if !$0 { state.ui.tvWakeOffer = nil } }
                ),
                presenting: state.ui.tvWakeOffer
            ) { tv in
                Button("Set Up") { state.ui.tvWake = tv }
                Button("Not Now", role: .cancel) {}
            } message: { tv in
                Text("Choosing \(tv.name) from any of your devices on its network can turn the TV on and open kōan. \(tv.name) shows a code to type here, once.")
            }
            .sheet(item: Binding(
                get: { state.ui.tvWake },
                set: { state.ui.tvWake = $0 }
            )) { tv in
                TvWakeSheet(target: tv)
                    .environment(state)
            }
    }
}

private struct TvWakeSheet: View {
    let target: TvWakeTarget
    @Environment(AppState.self) private var state
    @Environment(\.dismiss) private var dismiss
    @State private var phase = Phase.asking
    @State private var code = ""
    /// Bumped to ask the TV for a fresh code.
    @State private var attempt = 0
    /// The name of the Apple TV asked, as it calls itself: the screen the
    /// code should be on.
    @State private var asked: String?
    @FocusState private var focused: Bool

    private enum Phase: Equatable {
        case asking, entering, pairing, done
        case failed(String)
    }

    var body: some View {
        NavigationStack {
            KoanForm {
                Section {
                    content
                } header: {
                    KoanSectionHeader(target.name)
                } footer: {
                    Text("kōan pairs with the TV as the Remote app does. The TV lists it under Settings › Remotes and Devices, and Stop Waking in its menu here undoes it for all your devices.")
                        .koanText(.fine, .muted)
                }
            }
            .koanSheet()
            .navigationTitle(KoanTheme.label("Wake from Sleep"))
            .toolbar {
                KoanSheetAction(placement: phase == .done ? .confirmationAction : .cancellationAction) {
                    Button(phase == .done ? "Done" : "Cancel") { dismiss() }
                }
            }
        }
        #if os(macOS)
        .frame(minWidth: 420, minHeight: 260)
        #endif
        .task(id: attempt) { await begin() }
        .onDisappear {
            if phase != .done { state.engine.cancelTvPairing() }
        }
    }

    @ViewBuilder private var content: some View {
        switch phase {
        case .asking:
            HStack(spacing: 8) {
                ProgressView().controlSize(.small)
                Text("Asking \(target.name) for a code…").koanText(.body, .muted)
            }
        case .entering:
            Text("Type the code on \(asked ?? target.name). It waits about half a minute.")
                .koanText(.body, .muted)
            HStack {
                TextField("Code", text: $code, prompt: Text("0000"))
                    .verbatimEntry(.digits)
                    .koanField(code, prompt: "0000")
                    .focused($focused)
                    .onSubmit(finish)
                    .onChange(of: code) { _, typed in
                        if typed.filter(\.isNumber).count == 4 { finish() }
                    }
                Button("Pair", action: finish)
                    .koanButton(.prominent)
                    .disabled(code.filter(\.isNumber).count < 4)
            }
            .rowButtons()
        case .pairing:
            HStack(spacing: 8) {
                ProgressView().controlSize(.small)
                Text("Pairing…").koanText(.body, .muted)
            }
        case .done:
            Text("Choosing \(target.name) from any of your devices on its network now turns it on and opens kōan.")
                .koanText(.body, .muted)
        case let .failed(why):
            Text(why).koanText(.body, .bad)
            Button("Try Again") { attempt += 1 }
                .koanButton(.bordered)
        }
    }

    private func begin() async {
        phase = .asking
        code = ""
        do {
            asked = try await state.engine.beginTvPairing(id: target.id)
            phase = .entering
            focused = true
        } catch {
            phase = .failed(SettingsModel.describe(error))
        }
    }

    private func finish() {
        guard phase == .entering, code.filter(\.isNumber).count == 4 else { return }
        phase = .pairing
        let pin = code
        Task {
            do {
                try await state.engine.finishTvPairing(pin: pin)
                phase = .done
            } catch {
                phase = .failed(SettingsModel.describe(error))
            }
        }
    }
}
