@testable import Koan
import KoanFFI
import Testing

struct DeviceActionTests {
    @Test func aTuningGoesIntoTheFiltersAndNeverTakesTheCorrectionsPlace() {
        #expect(DeviceAction(name: "Warm", role: .tuning, active: "Warm", inChain: false) == .moveToFilters)
        #expect(DeviceAction(name: "HD 650", role: .correction, active: "HD 650", inChain: false) == .stopCorrecting)
        #expect(DeviceAction(name: "Warm", role: .tuning, active: nil, inChain: true) == .takeOutOfFilters)
        #expect(DeviceAction(name: "Warm", role: .tuning, active: "HD 650", inChain: false) == .addToFilters)
        #expect(DeviceAction(name: "HD 650", role: .baked, active: nil, inChain: false) == .correct)
    }
}
