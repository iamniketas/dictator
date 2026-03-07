// swift-tools-version: 5.10
import PackageDescription

let package = Package(
    name: "DictatorMac",
    platforms: [
        .macOS(.v14),
    ],
    products: [
        .executable(name: "DictatorMac", targets: ["DictatorMac"]),
    ],
    dependencies: [
        .package(url: "https://github.com/argmaxinc/WhisperKit.git", from: "0.13.0"),
        .package(url: "https://github.com/sparkle-project/Sparkle.git", from: "2.7.0"),
    ],
    targets: [
        .executableTarget(
            name: "DictatorMac",
            dependencies: [
                .product(name: "WhisperKit", package: "WhisperKit"),
                .product(name: "Sparkle", package: "Sparkle"),
            ],
            path: "Sources/DictatorMac",
            exclude: ["Resources"]
        ),
    ]
)
