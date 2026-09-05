import Foundation

public enum BenchError: Error, LocalizedError, Sendable {
  case invalid(String)
  public var errorDescription: String? {
    if case .invalid(let message) = self { return message }
    return nil
  }
}

public enum RenderMode: String, CaseIterable, Codable, Sendable, Identifiable {
  case native, temporal, spatial, bilinear
  public var id: String { rawValue }
  public var title: String { rawValue.capitalized }
}

public struct BenchConfiguration: Sendable, Equatable {
  public var background = true
  public var mode: RenderMode = .native
  public var scale: String = "1"
  public var rounds: Int = 1
  public var claudes: Int = 64
  public var lights: Int = 8
  public var particles: Int = 4096
  public var fill: Int = 0
  public init() {}
  public func arguments(command: String, output: URL) -> [String] {
    var result = [
      command, "--out", output.path, "--width", "2560", "--height", "1440",
      "--frames", "1200", "--seed", "21434",
    ]
    if command != "compare" {
      result += ["--mode", mode.rawValue, "--scale", mode == .native ? "1" : scale]
    }
    if command == "compare" { result += ["--rounds", String(rounds)] }
    if command == "stress" {
      result += [
        "--duration", "600", "--claudes", String(claudes), "--lights", String(lights),
        "--particles", String(particles), "--fill", String(fill),
      ]
    }
    if background { result.append("--background") }
    return result
  }
  public var stressMessage: Data {
    (try? JSONSerialization.data(
      withJSONObject: [
        "event": "configure", "claudes": claudes,
        "lights": lights, "particles": particles, "fill": fill,
      ], options: [.sortedKeys])).map { $0 + Data([10]) } ?? Data()
  }
}

/// Capture presentation behavior when the process starts. Later form edits must
/// not hide, show or activate windows for a different run configuration.
public struct RunPresentation: Sendable, Equatable {
  public enum LaunchBehavior: Sendable, Equatable {
    case stayInLauncher, hideLauncher, showStressPanel
  }
  public let background: Bool
  public let launchBehavior: LaunchBehavior
  public var activatesResults: Bool { !background }
  public var handlesLauncherEscape: Bool { background }
  public init(command: String, configuration: BenchConfiguration) {
    background = configuration.background
    launchBehavior =
      background ? .stayInLauncher : (command == "stress" ? .showStressPanel : .hideLauncher)
  }
}

public struct ChildEvent: Decodable, Sendable, Equatable {
  public let schemaVersion: Int
  public let event: String
  public let message: String?
  public let scene: String?
  public let progress: Double?
  public let renderFPS: Double?
  public let report: String?
  public let path: String?
  public let valid: Bool?
  enum CodingKeys: String, CodingKey {
    case schemaVersion = "schema_version"
    case event, message, scene, progress, report, path, valid
    case renderFPS = "render_fps"
  }
}

/// A complete event can arrive fragmented across reads. Unknown events are ignored;
/// malformed known messages poison the run even if a later event claims success.
public struct EventDecoder: Sendable {
  private var pending = Data()
  public private(set) var errors: [String] = []
  private let limit = 1_048_576
  public init() {}
  public mutating func append(_ data: Data, final: Bool = false) -> [ChildEvent] {
    pending.append(data)
    if pending.count > limit && !pending.contains(10) {
      errors.append("The renderer emitted an oversized message.")
      pending.removeAll()
      return []
    }
    var events: [ChildEvent] = []
    while let newline = pending.firstIndex(of: 10) {
      let line = Data(pending[..<newline])
      pending.removeSubrange(...newline)
      if let event = decode(line) { events.append(event) }
    }
    if final && !pending.isEmpty {
      if let event = decode(pending) { events.append(event) }
      pending.removeAll()
    }
    return events
  }
  private mutating func decode(_ line: Data) -> ChildEvent? {
    if line.allSatisfy({ $0 == 13 || $0 == 32 || $0 == 9 }) { return nil }
    guard line.count <= limit,
      let object = try? JSONSerialization.jsonObject(with: line) as? [String: Any],
      let name = object["event"] as? String
    else {
      errors.append("The renderer emitted malformed progress data.")
      return nil
    }
    guard ["started", "progress", "scene_complete", "complete", "error"].contains(name) else {
      return nil
    }
    guard let event = try? JSONDecoder().decode(ChildEvent.self, from: line),
      event.schemaVersion == 1,
      event.progress.map({ $0.isFinite && $0 >= 0 && $0 <= 1 }) ?? true,
      event.renderFPS.map({ $0.isFinite && $0 >= 0 }) ?? true
    else {
      errors.append("The renderer protocol is incompatible or invalid.")
      return nil
    }
    if event.event == "complete" && (event.report == nil || event.valid == nil) {
      errors.append("The renderer omitted its final report identity.")
      return nil
    }
    return event
  }
}

public enum ContainedPath {
  public static func resolve(_ path: String, in root: URL, relativeTo parent: URL? = nil) throws
    -> URL
  {
    let base = try canonical(root)
    let candidate = try canonical(
      path.hasPrefix("/")
        ? URL(fileURLWithPath: path) : (parent ?? base).appendingPathComponent(path))
    guard candidate.path.hasPrefix(base.path + "/") else {
      throw BenchError.invalid("A report references a file outside its run folder.")
    }
    return candidate
  }
  static func canonical(_ url: URL, depth: Int = 0) throws -> URL {
    guard depth < 32 else {
      throw BenchError.invalid("A report path contains a symbolic-link cycle.")
    }
    // Foundation standardization aliases /private/var back to /var. Walk
    // physical components ourselves, including existing symlink ancestors of
    // missing files, so an escaping reference cannot pass containment.
    var components: [String] = []
    for component in url.pathComponents.dropFirst() {
      if component == "." { continue }
      if component == ".." {
        if !components.isEmpty { components.removeLast() }
        continue
      }
      components.append(component)
      let path = "/" + components.joined(separator: "/")
      if let target = try? FileManager.default.destinationOfSymbolicLink(atPath: path) {
        let parent = "/" + components.dropLast().joined(separator: "/")
        let next = URL(fileURLWithPath: target.hasPrefix("/") ? target : parent + "/" + target)
        components = Array(try canonical(next, depth: depth + 1).pathComponents.dropFirst())
      }
    }
    return URL(fileURLWithPath: "/" + components.joined(separator: "/"))
  }
  public static func regularFile(_ path: String, in root: URL, relativeTo parent: URL? = nil) throws
    -> URL
  {
    let url = try resolve(path, in: root, relativeTo: parent)
    let values = try url.resourceValues(forKeys: [.isRegularFileKey, .isSymbolicLinkKey])
    guard values.isRegularFile == true && values.isSymbolicLink != true else {
      throw BenchError.invalid("A report artifact is not a regular file.")
    }
    return url
  }
}
