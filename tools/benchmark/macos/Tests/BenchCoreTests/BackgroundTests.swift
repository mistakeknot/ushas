import Foundation
import Testing

@testable import BenchCore

func backgroundFixture(profile: String = "claude-lab-offscreen-v1", background: Any? = true)
  -> [String: Any]
{
  var object = fixture(profile: profile)
  var config = object["config"] as! [String: Any]
  config["background"] = background
  object["config"] = config
  return object
}

@Test func backgroundArgumentsAreValuelessAndWindowedRemainsAvailable() throws {
  var config = BenchConfiguration()
  for command in ["benchmark", "compare", "stress"] {
    let args = config.arguments(command: command, output: URL(fileURLWithPath: "/tmp/new-run"))
    #expect(args.last == "--background")
    #expect(args.filter { $0 == "--background" }.count == 1)
    #expect(!args.contains("true"))
  }
  config.background = false
  #expect(
    !config.arguments(command: "benchmark", output: URL(fileURLWithPath: "/tmp/new-run")).contains(
      "--background"))
  let configure = try JSONSerialization.jsonObject(with: config.stressMessage) as! [String: Any]
  #expect(configure["background"] == nil, "An active renderer's target cannot be reconfigured")
}

@Test func runPresentationRemainsBoundToTheLaunchedConfiguration() {
  var config = BenchConfiguration()
  for command in ["benchmark", "compare", "stress"] {
    let launched = RunPresentation(command: command, configuration: config)
    config.background = false
    #expect(launched.launchBehavior == .stayInLauncher)
    #expect(!launched.activatesResults)
    #expect(launched.handlesLauncherEscape)
    let windowed = RunPresentation(command: command, configuration: config)
    #expect(windowed.launchBehavior == (command == "stress" ? .showStressPanel : .hideLauncher))
    #expect(windowed.activatesResults)
    #expect(!windowed.handlesLauncherEscape)
    config.background = true
  }
}

@Test func backgroundProfilesRequireExplicitConsistentTargetsAndLegacyDefaultsAreWindowed() throws {
  func decode(_ value: [String: Any]) throws -> BenchReport {
    try JSONDecoder().decode(BenchReport.self, from: JSONSerialization.data(withJSONObject: value))
  }
  let legacy = try decode(fixture())
  #expect(legacy.config?.background == false)
  #expect(legacy.executionLabel == "Windowed")
  #expect(legacy.score == 144)
  let background = try decode(backgroundFixture())
  #expect(background.executionLabel == "Background")
  #expect(background.standard)
  #expect(background.score == 144)
  for object in [
    backgroundFixture(background: nil), backgroundFixture(background: false),
    backgroundFixture(profile: "claude-lab-standard-v1"),
  ] {
    #expect(try decode(object).score == nil)
  }
  for invalid in [NSNull(), "true", 1] as [Any] {
    #expect(throws: (any Error).self) { try decode(backgroundFixture(background: invalid)) }
  }
  let custom = try decode(backgroundFixture(profile: "custom"))
  #expect(custom.score == 144)
  #expect(!custom.standard)
}

@Test func customComparisonCannotMixWindowedAndBackgroundChildren() throws {
  let root = try temporaryDirectory()
  defer { try? FileManager.default.removeItem(at: root) }
  let presets = [
    ("native", 1.0), ("temporal", 1.0), ("temporal", 2.0 / 3.0),
    ("temporal", 0.5), ("spatial", 0.5), ("bilinear", 0.5),
  ]
  var arms: [[String: Any]] = []
  for (index, preset) in presets.enumerated() {
    let directory = root.appendingPathComponent("arm-\(index)")
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: false)
    var child = backgroundFixture(profile: "custom")
    var config = child["config"] as! [String: Any]
    config["mode"] = preset.0
    config["scale"] = preset.1
    child["config"] = config
    try JSONSerialization.data(withJSONObject: child).write(
      to: directory.appendingPathComponent("result.json"))
    arms.append([
      "label": "Arm \(index)", "mode": preset.0, "scale": preset.1, "round": 1,
      "report": "arm-\(index)/result.json", "valid": true, "render_fps": 144.0, "captures": [],
    ])
  }
  var parent = backgroundFixture(profile: "custom")
  parent["kind"] = "compare"
  parent["rounds"] = 1
  parent["arms"] = arms
  parent["render_fps"] = NSNull()
  let file = root.appendingPathComponent("result.json")
  try JSONSerialization.data(withJSONObject: parent).write(to: file)
  #expect(try LoadedReport.load(file).accepted)
  try JSONSerialization.data(
    withJSONObject: backgroundFixture(profile: "custom", background: false)
  )
  .write(to: root.appendingPathComponent("arm-0/result.json"))
  let mixed = try LoadedReport.load(file)
  #expect(!mixed.accepted)
  #expect(mixed.arms.first?.score == nil)
}

@Test func offlineExportNamesTheExecutionTarget() throws {
  let root = try temporaryDirectory()
  defer { try? FileManager.default.removeItem(at: root) }
  let run = root.appendingPathComponent("run")
  try FileManager.default.createDirectory(at: run, withIntermediateDirectories: false)
  let file = run.appendingPathComponent("result.json")
  try JSONSerialization.data(withJSONObject: backgroundFixture()).write(to: file)
  let report = try LoadedReport.load(file)
  let store = try RunStore(root: root.appendingPathComponent("history"))
  let destination = root.appendingPathComponent("export")
  try store.export(report, to: destination)
  let html = try String(
    contentsOf: destination.appendingPathComponent("index.html"), encoding: .utf8)
  #expect(html.contains("Execution: Background"))
  #expect(html.contains("claude-lab-offscreen-v1"))
}
