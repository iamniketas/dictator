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
        .package(url: "https://github.com/argmaxinc/WhisperKit.git", from: "0.9.0"),
    ],
    targets: [
        .executableTarget(
            name: "DictatorMac",
            dependencies: [
                .product(name: "WhisperKit", package: "WhisperKit"),
            ],
            path: "Sources/DictatorMac",
            exclude: ["Resources"]
        ),
    ]
)
