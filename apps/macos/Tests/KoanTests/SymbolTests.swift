import AppKit
@testable import Koan
import Testing

/// Symbols drawn for layers are cached by the colours they resolve to.
@MainActor
struct SymbolTests {
    private func dynamic(_ colour: NSColor) -> NSColor {
        NSColor(name: nil) { _ in colour }
    }

    @Test func dynamicColoursThatResolveApartDrawApart() throws {
        let appearance = try #require(NSAppearance(named: .aqua))
        let red = Symbol.image("circle.fill", size: 12, colours: [dynamic(.systemRed)], appearance: appearance)
        let blue = Symbol.image("circle.fill", size: 12, colours: [dynamic(.systemBlue)], appearance: appearance)
        #expect(red != nil)
        #expect(red !== blue)
    }

    @Test func theSameColourIsDrawnOnce() throws {
        let appearance = try #require(NSAppearance(named: .aqua))
        let first = Symbol.image("circle.fill", size: 12, colours: [.systemGreen], appearance: appearance)
        let again = Symbol.image("circle.fill", size: 12, colours: [.systemGreen], appearance: appearance)
        #expect(first === again)
    }
}
