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
                .unsafeFlags(["-L../../target/release"]),
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
