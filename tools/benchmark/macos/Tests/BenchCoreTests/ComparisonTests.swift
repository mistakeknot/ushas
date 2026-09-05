import Foundation
import Testing

@testable import BenchCore

@Test func comparisonRequiresEveryCompatibleChildAndWithholdsInvalidExportScores() throws {
  let root = try temporaryDirectory()
  defer { try? FileManager.default.removeItem(at: root) }
  let run = root.appendingPathComponent("run")
  try FileManager.default.createDirectory(at: run, withIntermediateDirectories: false)
  let presets = [
    ("native", 1.0), ("temporal", 1.0), ("temporal", 2.0 / 3.0), ("temporal", 0.5),
    ("spatial", 0.5), ("bilinear", 0.5),
  ]
  var arms: [[String: Any]] = []
  for (index, preset) in presets.enumerated() {
    let directory = run.appendingPathComponent("arm-\(index)")
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: false)
    var child = fixture()
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
  var object = fixture()
  object["kind"] = "compare"
  object["rounds"] = 1
  object["arms"] = arms
  object["render_fps"] = NSNull()
  let file = run.appendingPathComponent("result.json")
  try JSONSerialization.data(withJSONObject: object).write(to: file)
  let accepted = try LoadedReport.load(file)
  #expect(accepted.accepted)
  #expect(accepted.arms.allSatisfy { $0.score == 144 })
  var incompatible = fixture()
  incompatible["source_revision"] = "different-build"
  try JSONSerialization.data(withJSONObject: incompatible).write(
    to: run.appendingPathComponent("arm-0/result.json"))
  let rejected = try LoadedReport.load(file)
  #expect(!rejected.accepted)
  #expect(rejected.arms.first?.score == nil)
  let store = try RunStore(root: root.appendingPathComponent("history"))
  let destination = root.appendingPathComponent("export")
  try store.export(rejected, to: destination)
  let html = try String(
    contentsOf: destination.appendingPathComponent("index.html"), encoding: .utf8)
  #expect(html.contains("No valid score"))
  #expect(!html.contains("144.0"))
}
