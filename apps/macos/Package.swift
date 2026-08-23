// swift-tools-version: 6.0
import PackageDescription

// The Rust static lib and the generated bindings are build products, not
// sources — `just macos-ffi` puts them in place before `swift build` runs.
let package = Package(
    name: "Koan",
    platforms: [.macOS(.v14)],
    targets: [
        .systemLibrary(name: "koan_ffiFFI", path: "Sources/koan_ffiFFI"),
        .target(
            name: "KoanFFI",
            dependencies: ["koan_ffiFFI"],
            path: "Sources/KoanFFI"
        ),
        .executableTarget(
            name: "Koan",
            dependencies: ["KoanFFI"],
            path: "Sources/Koan",
            linkerSettings: [
                // Both, because a universal build lipo's its output somewhere
                // else: `just macos-ffi <targets>` writes
                // target/universal/release, while a single-arch build writes
                // target/release. The linker takes the first that exists, and a
                // missing search path is not an error — so with only the
                // single-arch path here, the release DMG failed to link with
                // "library 'koan_ffi' not found" while local builds were fine.
                .unsafeFlags([
                    "-L../../target/universal/release",
                    "-L../../target/release",
                ]),
                .linkedLibrary("koan_ffi"),
                .linkedFramework("CoreAudio"),
                .linkedFramework("AudioToolbox"),
                .linkedFramework("AudioUnit"),
                .linkedFramework("CoreFoundation"),
                .linkedFramework("Security"),
                .linkedFramework("SystemConfiguration"),
            ]
        ),
    ]
)
