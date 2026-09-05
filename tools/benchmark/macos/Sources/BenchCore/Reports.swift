import Foundation

public struct CaptureReference: Decodable, Sendable, Identifiable {
  public let scene: String
  public let tick: Int
  public let path: String
  public var id: String { "\(scene):\(tick):\(path)" }
  public var pairingKey: String { "\(scene):\(tick)" }
}
public struct SceneReport: Decodable, Sendable, Identifiable {
  public let scene: String
  public let valid: Bool
  public let frames: Int
  public let elapsedSeconds: Double?
  public let renderFPS: Double?
  public let errors: [String]
  public var id: String { scene }
  enum CodingKeys: String, CodingKey {
    case scene, valid, frames, errors
    case renderFPS = "render_fps"
    case elapsedSeconds = "elapsed_seconds"
  }
}
public struct ArmReport: Decodable, Sendable, Identifiable {
  public let label: String
  public let mode: String
  public let scale: Double
  public let round: Int
  public let report: String
  public let valid: Bool
  public let renderFPS: Double?
  public let captures: [CaptureReference]?
  public var id: String { "\(round):\(label)" }
  enum CodingKeys: String, CodingKey {
    case label, mode, scale, round, report, valid, captures
    case renderFPS = "render_fps"
  }
}
public struct ReportConfiguration: Decodable, Sendable {
  public struct Load: Decodable, Sendable {
    public let claudes: Int?
    public let lights: Int?
    public let particles: Int?
    public let fill: Int
  }
  public let width: Int
  public let mode: String?
  public let scale: Double?
  public let height: Int
  public let frames: Int
  public let seed: Int
  public let scene: String?
  public let load: Load
  public let background: Bool
  enum CodingKeys: String, CodingKey {
    case width, mode, scale, height, frames, seed, scene, load, background
  }
  public init(from decoder: Decoder) throws {
    let values = try decoder.container(keyedBy: CodingKeys.self)
    width = try values.decode(Int.self, forKey: .width)
    mode = try values.decodeIfPresent(String.self, forKey: .mode)
    scale = try values.decodeIfPresent(Double.self, forKey: .scale)
    height = try values.decode(Int.self, forKey: .height)
    frames = try values.decode(Int.self, forKey: .frames)
    seed = try values.decode(Int.self, forKey: .seed)
    scene = try values.decodeIfPresent(String.self, forKey: .scene)
    load = try values.decode(Load.self, forKey: .load)
    // Missing is the legacy windowed default; present null/mistyped values are invalid.
    background =
      values.contains(.background) ? try values.decode(Bool.self, forKey: .background) : false
  }
  public var legal: Bool {
    guard let mode, RenderMode(rawValue: mode) != nil, let scale, scale.isFinite,
      [1.0, 2.0 / 3.0, 0.5].contains(where: { abs($0 - scale) < 1e-6 }),
      mode != "native" || scale == 1
    else { return false }
    return width > 0 && height > 0 && frames > 0 && seed >= 0
      && (scene == nil || ["materials", "geometry", "lighting"].contains(scene!))
  }
  public var standard: Bool {
    width == 2560 && height == 1440 && frames == 1200 && seed == 21434 && scene == nil
      && load.claudes == nil && load.lights == nil && load.particles == nil && load.fill == 0
  }
}
public struct PairedSummary: Decodable, Sendable, Identifiable {
  public let label: String
  public let valid: Bool
  public let qualified: Bool?
  public let timeReduction: Double?
  public let ci95: [Double]?
  public let performanceGate: String?
  public var id: String { label }
  enum CodingKeys: String, CodingKey {
    case label, valid, qualified, ci95
    case timeReduction = "time_reduction"
    case performanceGate = "performance_gate"
  }
}
public struct BenchReport: Decodable, Sendable {
  public let schemaVersion: Int
  public let kind: String
  public let valid: Bool
  public let stopped: Bool?
  public let errors: [String]?
  public let profileVersion: String
  public let sourceRevision: String?
  public let binarySHA256: String?
  public let startedUTC: String?
  public let renderFPS: Double?
  public let config: ReportConfiguration?
  public let scenes: [SceneReport]?
  public let captures: [CaptureReference]?
  public let arms: [ArmReport]?
  public let pairedSummaries: [PairedSummary]?
  public let rounds: Int?
  enum CodingKeys: String, CodingKey {
    case schemaVersion = "schema_version"
    case profileVersion = "profile_version"
    case sourceRevision = "source_revision"
    case binarySHA256 = "binary_sha256"
    case startedUTC = "started_utc"
    case renderFPS = "render_fps"
    case kind, valid, stopped, errors, config, scenes, captures, arms, rounds
    case pairedSummaries = "paired_summaries"
  }
  public var compatible: Bool {
    schemaVersion == 1 && ["benchmark", "compare", "stress", "capture"].contains(kind)
      && ["claude-lab-standard-v1", "claude-lab-offscreen-v1", "custom"].contains(profileVersion)
  }
  public var executionLabel: String { config?.background == true ? "Background" : "Windowed" }
  private var standardProfileMatchesTarget: Bool {
    profileVersion
      == (config?.background == true ? "claude-lab-offscreen-v1" : "claude-lab-standard-v1")
  }
  public var standard: Bool {
    standardProfileMatchesTarget && config?.standard == true && config?.legal == true
  }
  public var failure: String? {
    if !compatible { return "This report uses an unsupported benchmark profile." }
    if stopped == true { return "The run was stopped. Its artifacts are preserved." }
    if !valid { return errors?.first ?? "This run did not meet the measurement checks." }
    guard stopped != nil, let errors, let config, config.legal,
      let revision = sourceRevision, !revision.isEmpty,
      let hash = binarySHA256, hash.count == 64, hash.allSatisfy({ $0.isHexDigit })
    else {
      return "The report is missing required configuration or validation metadata."
    }
    if let error = errors.first { return error }
    if profileVersion != "custom" && (!standardProfileMatchesTarget || !config.standard) {
      return "The standard profile has incompatible run settings."
    }
    if kind == "benchmark" {
      guard let scenes, !scenes.isEmpty, let score = renderFPS, score.isFinite, score > 0,
        scenes.allSatisfy({ scene in
          guard scene.valid, scene.errors.isEmpty, scene.frames == config.frames,
            let fps = scene.renderFPS, fps.isFinite, fps > 0,
            let elapsed = scene.elapsedSeconds, elapsed.isFinite, elapsed > 0
          else { return false }
          return abs(Double(scene.frames) / elapsed - fps) <= max(1e-6, fps * 1e-6)
        })
      else { return "The report has no consistent completed-render measurement." }
      let expected = config.scene.map { [$0] } ?? ["materials", "geometry", "lighting"]
      if Set(scenes.map(\.scene)) != Set(expected) || scenes.count != expected.count {
        return "The report is missing a requested scene."
      }
      let calculated = exp(
        scenes.compactMap(\.renderFPS).map(log).reduce(0, +) / Double(scenes.count))
      if abs(calculated - score) > max(1e-6, score * 1e-6) {
        return "The score disagrees with its scene measurements."
      }
    }
    if kind == "compare" {
      guard let rounds, [1, 4].contains(rounds), let arms, arms.count == rounds * 6,
        Set(arms.map(\.id)).count == arms.count
      else { return "The comparison is missing its complete set of arms." }
      let expected = Set([
        "native:1000000", "temporal:1000000", "temporal:666667", "temporal:500000",
        "spatial:500000", "bilinear:500000",
      ])
      for round in 1...rounds {
        let cohort = arms.filter { $0.round == round }
        guard cohort.allSatisfy({ $0.scale.isFinite && $0.scale > 0 && $0.scale <= 1 }) else {
          return "A comparison arm uses an invalid scale."
        }
        let actual = Set(cohort.map { "\($0.mode):\(Int(($0.scale * 1_000_000).rounded()))" })
        if cohort.count != 6 || actual != expected {
          return "The comparison arm configuration is incompatible."
        }
      }
    }
    return nil
  }
  public var score: Double? { failure == nil && kind == "benchmark" ? renderFPS : nil }
}
public struct LoadedArm: Sendable, Identifiable {
  public let arm: ArmReport
  public let child: BenchReport?
  public let problem: String?
  public var id: String { arm.id }
  public var score: Double? { problem == nil && arm.valid ? child?.score : nil }
}
public struct LoadedReport: Sendable, Identifiable {
  public let url: URL
  public let report: BenchReport
  public let arms: [LoadedArm]
  public let problem: String?
  public var id: String { url.path }
  public var root: URL { url.deletingLastPathComponent() }
  public var score: Double? { problem == nil ? report.score : nil }
  public var accepted: Bool { problem == nil && report.failure == nil }
  public static func load(_ url: URL) throws -> LoadedReport {
    let root = url.deletingLastPathComponent().standardizedFileURL.resolvingSymlinksInPath()
    let resolved = try ContainedPath.regularFile(url.path, in: root)
    let bytes = try Data(contentsOf: resolved)
    guard bytes.count < 64 * 1024 * 1024 else {
      throw BenchError.invalid("The result file is too large.")
    }
    let report = try JSONDecoder().decode(BenchReport.self, from: bytes)
    var problem = report.failure
    var loaded: [LoadedArm] = []
    for arm in report.arms ?? [] {
      do {
        let childURL = try ContainedPath.regularFile(arm.report, in: root)
        let child = try JSONDecoder().decode(BenchReport.self, from: Data(contentsOf: childURL))
        let issue = !arm.valid ? "This arm was not valid." : child.failure
        if child.kind != "benchmark" || child.score == nil || child.config?.mode != arm.mode
          || child.config?.scale.map({ abs($0 - arm.scale) < 1e-6 }) != true
          || child.sourceRevision != report.sourceRevision
          || child.binarySHA256 != report.binarySHA256
          || child.profileVersion != report.profileVersion
          || child.config?.background != report.config?.background
          || child.config?.width != report.config?.width
          || child.config?.height != report.config?.height
          || child.config?.seed != report.config?.seed
          || child.config?.frames != report.config?.frames
          || arm.renderFPS.map({
            abs($0 - (child.score ?? 0)) < max(1e-6, (child.score ?? 0) * 1e-6)
          }) != true
        {
          throw BenchError.invalid("A comparison arm has an incompatible result.")
        }
        loaded.append(LoadedArm(arm: arm, child: child, problem: issue))
      } catch {
        loaded.append(LoadedArm(arm: arm, child: nil, problem: error.localizedDescription))
      }
    }
    if report.kind == "compare" && (loaded.isEmpty || loaded.contains(where: { $0.problem != nil }))
    {
      problem =
        "The comparison contains an incomplete or invalid arm. No valid comparison is available."
    }
    if report.kind == "compare" && !report.valid { problem = report.failure }
    return LoadedReport(url: resolved, report: report, arms: loaded, problem: problem)
  }
  public func invalidated(_ reason: String) -> LoadedReport {
    LoadedReport(url: url, report: report, arms: arms, problem: reason)
  }
  public func imageURL(_ capture: CaptureReference) throws -> URL {
    try ContainedPath.regularFile(capture.path, in: root)
  }
}
