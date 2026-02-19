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
    targets: [
        .executableTarget(
            name: "DictatorMac",
            path: "Sources/DictatorMac",
            exclude: ["Resources"]
        ),
    ]
)
