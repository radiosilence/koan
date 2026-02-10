// swift-tools-version: 6.0
import PackageDescription

let rustLibPath = "../target/release"

let package = Package(
    name: "Koan",
    platforms: [.macOS(.v14)],
    targets: [
        .systemLibrary(
            name: "KoanRust",
            path: "Sources/KoanRust"
        ),
        .executableTarget(
            name: "Koan",
            dependencies: ["KoanRust"],
            path: "Sources/Koan",
            linkerSettings: [
                .unsafeFlags([
                    "-L\(rustLibPath)",
                    "-lkoan_ffi",
                ]),
            ]
        ),
    ]
)
