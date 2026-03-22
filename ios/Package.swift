// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "Koan",
    platforms: [.iOS(.v17), .macOS(.v14)],
    products: [
        .library(name: "Koan", targets: ["Koan"]),
    ],
    targets: [
        .target(
            name: "Koan",
            path: "Sources/Koan",
            exclude: ["KoanApp.swift"]
        ),
    ]
)
