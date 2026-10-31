/// Await `read` off the main actor.
///
/// KoanFFI is built with `NonisolatedNonsendingByDefault` (see Package.swift),
/// so an engine call runs on its caller's actor: that is what makes commands
/// join the engine's lane in the order they were made. A read awaited from the
/// main actor therefore decodes its answer there too, and a listing of
/// thousands of records is a visible hitch. Reads of more than a handful of
/// rows go through this, or through `Task.detached` as the pages that load
/// whole listings do.
@concurrent
func offMain<T: Sendable>(_ read: @Sendable () async throws -> T) async rethrows -> T {
    try await read()
}
