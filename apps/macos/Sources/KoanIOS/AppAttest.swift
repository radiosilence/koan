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

    /// The value a completion handler gives, waited for.
    private func wait<T>(_ start: (@escaping (T?, Error?) -> Void) -> Void) -> T? {
        let semaphore = DispatchSemaphore(value: 0)
        nonisolated(unsafe) var result: T?
        start { value, error in
            if let error { NSLog("koan: App Attest: \(error.localizedDescription)") }
            result = value
            semaphore.signal()
        }
        semaphore.wait()
        return result
    }
}
