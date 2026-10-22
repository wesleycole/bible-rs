// swift-tools-version:5.9
// linger-core for Swift. LingerFFI.xcframework and Sources/LingerCore/Generated/ come from
// scripts/build-ios-core.sh and are not checked in.
import PackageDescription

let package = Package(
    name: "LingerCore",
    // Keep in step with scripts/build-ios-core.sh and ../project.yml.
    platforms: [.iOS("26.1"), .macOS(.v14)],
    products: [.library(name: "LingerCore", targets: ["LingerCore"])],
    targets: [
        .binaryTarget(name: "LingerFFI", path: "LingerFFI.xcframework"),
        .target(
            name: "LingerCore",
            dependencies: ["LingerFFI"],
            // On macOS linger-core keeps API keys in the Keychain (the `keyring` crate).
            linkerSettings: [
                .linkedFramework("Security", .when(platforms: [.macOS])),
                .linkedFramework("CoreFoundation", .when(platforms: [.macOS])),
            ]
        ),
        .testTarget(name: "LingerCoreTests", dependencies: ["LingerCore"]),
    ]
)
