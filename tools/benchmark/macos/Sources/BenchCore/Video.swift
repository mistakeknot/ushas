import CryptoKit
import Darwin
import Foundation

public enum VideoChapter: String, CaseIterable, Sendable, Identifiable {
  case all, materials, geometry, lighting
  public var id: String { rawValue }
  public var title: String {
    self == .all ? "All chapters · 30 seconds" : "\(rawValue.capitalized) · 10 seconds"
  }
}

public struct VideoMode: Sendable, Identifiable, Hashable {
  public let mode: RenderMode
  public let scale: String
  public var id: String { "\(mode.rawValue):\(scale)" }
  public var title: String {
    mode == .native
      ? "Native · Full · MSAA 4×"
      : "\(mode.title) · \(scale == "1" ? "Full" : scale == "1/2" ? "Half" : "Two-thirds")"
  }
  public init(mode: RenderMode, scale: String) {
    self.mode = mode
    self.scale = mode == .native ? "1" : scale
  }
  public static let comparison: [Self] = [
    .init(mode: .native, scale: "1"), .init(mode: .temporal, scale: "1"),
    .init(mode: .temporal, scale: "2/3"), .init(mode: .temporal, scale: "1/2"),
    .init(mode: .spatial, scale: "1/2"), .init(mode: .bilinear, scale: "1/2"),
  ]
}

public struct VideoReport: Decodable, Sendable {
  public let path: String
  public let width: Int
  public let height: Int
  public let fps: Int
  public let simulationHz: Int
  public let frameCount: Int
  public let durationSeconds: Double
  public let codec: String
  public let bitrate: Int
  public let colorSpace: String
  public let sha256: String
  public var durationLabel: String { "\(Int(durationSeconds)) seconds" }
  enum CodingKeys: String, CodingKey {
    case path, width, height, fps, codec, bitrate, sha256
    case simulationHz = "simulation_hz"
    case frameCount = "frame_count"
    case durationSeconds = "duration_seconds"
    case colorSpace = "color_space"
  }
  func matches(_ config: ReportConfiguration) -> Bool {
    let count = config.scene == nil ? 1800 : 600
    return width == 2560 && height == 1440 && fps == 60 && simulationHz == 120
      && frameCount == count && durationSeconds == Double(count) / 60
      && codec == "h264" && bitrate == 30_000_000 && colorSpace == "rec709"
      && path == "video.mp4" && sha256.count == 64 && sha256.allSatisfy { $0.isHexDigit }
      && config.background && config.width == width && config.height == height
      && config.frames == 1200
      && config.load.claudes == nil && config.load.lights == nil && config.load.particles == nil
      && config.load.fill == 0
  }
}

public enum VideoFile {
  public static func hash(_ url: URL) throws -> String {
    let file = try FileHandle(forReadingFrom: url)
    defer { try? file.close() }
    var hash = SHA256()
    while let data = try file.read(upToCount: 1_048_576), !data.isEmpty { hash.update(data: data) }
    return hash.finalize().map { String(format: "%02x", $0) }.joined()
  }
}

/// Captured only after NSSavePanel has supplied replacement confirmation.
public struct VideoDestination: Sendable {
  public let url: URL
  private let existingFingerprint: String?
  private static func fingerprint(at url: URL) throws -> String? {
    var info = stat()
    if lstat(url.path, &info) != 0 {
      if errno == ENOENT { return nil }
      throw BenchError.invalid(
        "The destination could not be inspected: \(String(cString: strerror(errno)))")
    }
    guard info.st_mode & S_IFMT == S_IFREG else {
      throw BenchError.invalid("Choose a regular MP4 file, not a folder or symbolic link.")
    }
    return
      "\(info.st_dev):\(info.st_ino):\(info.st_size):\(info.st_mtimespec.tv_sec):\(info.st_mtimespec.tv_nsec):\(info.st_ctimespec.tv_sec):\(info.st_ctimespec.tv_nsec)"
  }
  public init(url: URL) throws {
    self.url = url.standardizedFileURL
    existingFingerprint = try Self.fingerprint(at: self.url)
  }
  public func publish(source: URL, checkCancellation: () throws -> Void = {}) throws {
    let fm = FileManager.default
    let temporary = url.deletingLastPathComponent().appendingPathComponent(
      ".ushas-video-\(UUID().uuidString).mp4")
    defer { try? fm.removeItem(at: temporary) }
    try checkCancellation()
    let input = try FileHandle(forReadingFrom: source)
    defer { try? input.close() }
    let fd = open(temporary.path, O_WRONLY | O_CREAT | O_EXCL, S_IRUSR | S_IWUSR)
    guard fd >= 0 else {
      throw BenchError.invalid("The temporary video file could not be created.")
    }
    let file = FileHandle(fileDescriptor: fd, closeOnDealloc: true)
    defer { try? file.close() }
    while let data = try input.read(upToCount: 1_048_576), !data.isEmpty {
      try checkCancellation()
      try file.write(contentsOf: data)
    }
    try checkCancellation()
    try file.synchronize()
    try file.close()
    guard try Self.fingerprint(at: url) == existingFingerprint else {
      throw BenchError.invalid(
        "The destination changed while rendering. Choose it again to confirm replacement.")
    }
    try checkCancellation()
    let result =
      existingFingerprint == nil ? link(temporary.path, url.path) : rename(temporary.path, url.path)
    guard result == 0 else {
      throw BenchError.invalid(
        "The video could not be published: \(String(cString: strerror(errno)))")
    }
  }
}
