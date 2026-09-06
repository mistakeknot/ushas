// swift-tools-version: 6.2
import PackageDescription

let package = Package(
  name: "UshasBench",
  platforms: [.macOS("26.0")],
  products: [
    .executable(name: "UshasBench", targets: ["UshasBench"]),
    .executable(name: "ushas-video-encoder", targets: ["UshasVideoEncoder"]),
  ],
  targets: [
    .target(name: "BenchCore"),
    .executableTarget(name: "UshasBench", dependencies: ["BenchCore"]),
    .testTarget(name: "BenchCoreTests", dependencies: ["BenchCore"]),
    .target(name: "VideoEncoderCore"),
    .executableTarget(name: "UshasVideoEncoder", dependencies: ["VideoEncoderCore"]),
    .testTarget(name: "VideoEncoderTests", dependencies: ["VideoEncoderCore"]),
  ]
)
