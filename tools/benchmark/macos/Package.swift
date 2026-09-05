// swift-tools-version: 6.2
import PackageDescription

let package = Package(
  name: "UshasBench",
  platforms: [.macOS("26.0")],
  products: [.executable(name: "UshasBench", targets: ["UshasBench"])],
  targets: [
    .target(name: "BenchCore"),
    .executableTarget(name: "UshasBench", dependencies: ["BenchCore"]),
    .testTarget(name: "BenchCoreTests", dependencies: ["BenchCore"]),
  ]
)
