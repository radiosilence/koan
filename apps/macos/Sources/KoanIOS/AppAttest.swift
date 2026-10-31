import DeviceCheck
import Foundation
import KoanFFI

/// App Attest for kōan's push relay: Apple's word that a registration comes
/// from this app on a real device, which the relay needs before it registers
/// a token on the production gateway. The engine calls it from a background
/// thread and waits; the system answers through completion handlers, so each
/// call blocks on one.
final class AppAttest: PushAttester, @unchecked Sendable {
    func attest(clientDataHash: Data) -> AttestedKey? {
        let service = DCAppAttestService.shared
        guard service.isSupported else { return nil }
        guard let keyId = wait({ done in service.generateKey { done($0, $1) } }),
              let id = Data(base64Encoded: keyId),
              let attestation = wait({ done in
                  service.attestKey(keyId, clientDataHash: clientDataHash) { done($0, $1) }
              })
        else { return nil }
        return AttestedKey(keyId: id, attestation: attestation)
    }

    func assert(keyId: Data, clientDataHash: Data) -> Data? {
        let service = DCAppAttestService.shared
        guard service.isSupported else { return nil }
        return wait { done in
            service.generateAssertion(keyId.base64EncodedString(), clientDataHash: clientDataHash) {
                done($0, $1)
            }
        }
    }

    /// How long one answer is waited for. Attesting asks Apple's servers,
    /// and a system that never calls back must not hold the relay's
    /// registration, and the thread it runs on, for ever.
    private static let patience: DispatchTimeInterval = .seconds(30)

    /// The value a completion handler gives, waited for; nil past `patience`.
    private func wait<T: Sendable>(_ start: (@escaping @Sendable (T?, Error?) -> Void) -> Void) -> T? {
        let answer = Answer<T>()
        start { value, error in
            if let error { NSLog("koan: App Attest: \(error.localizedDescription)") }
            answer.set(value)
        }
        guard answer.semaphore.wait(timeout: .now() + Self.patience) == .success else {
            NSLog("koan: App Attest: no answer in \(Self.patience)")
            return nil
        }
        return answer.value
    }
}

/// An answer that may arrive after its waiter has given up, so it is held
/// under a lock rather than in the waiter's frame.
private final class Answer<T: Sendable>: @unchecked Sendable {
    let semaphore = DispatchSemaphore(value: 0)
    private let lock = NSLock()
    private var stored: T?

    var value: T? { lock.withLock { stored } }

    func set(_ value: T?) {
        lock.withLock { stored = value }
        semaphore.signal()
    }
}
