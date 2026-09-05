import Foundation
import Testing

@testable import BenchCore

@Test func fragmentedEventsAndUnknownExtensions() throws {
  var decoder = EventDecoder()
  #expect(decoder.append(Data("{\"schema_version\":1,\"event\":\"pro".utf8)).isEmpty)
  let events = decoder.append(
    Data("gress\",\"progress\":0.4}\n{\"event\":\"future_event\"}\n".utf8))
  #expect(events.count == 1)
  #expect(events.first?.progress == 0.4)
  #expect(decoder.errors.isEmpty)
}
@Test func malformedKnownEventCannotBeRedeemedByCompletion() {
  var decoder = EventDecoder()
  _ = decoder.append(
    Data(
      "{\"schema_version\":2,\"event\":\"progress\"}\n{\"schema_version\":1,\"event\":\"complete\",\"valid\":true,\"report\":\"/tmp/result.json\"}\n"
        .utf8))
  #expect(!decoder.errors.isEmpty)
}
@Test func nativeConfigurationForcesFullScale() {
  var config = BenchConfiguration()
  config.scale = "1/2"
  let args = config.arguments(command: "benchmark", output: URL(fileURLWithPath: "/tmp/example"))
  #expect(args[args.firstIndex(of: "--scale")! + 1] == "1")
}
func temporaryDirectory() throws -> URL {
  let url = FileManager.default.temporaryDirectory.appendingPathComponent(
    "ushas-swift-test-" + UUID().uuidString)
  try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
  return url
}
func fixture(
  valid: Bool = true, stopped: Bool = false, schema: Int = 1,
  profile: String = "claude-lab-standard-v1"
) -> [String: Any] {
  [
    "schema_version": schema, "kind": "benchmark", "valid": valid, "stopped": stopped, "errors": [],
    "profile_version": profile, "render_fps": 144.0, "source_revision": "cpu-fixture",
    "binary_sha256": String(repeating: "a", count: 64),
    "config": [
      "mode": "native", "scale": 1.0, "width": 2560, "height": 1440, "frames": 1200, "seed": 21434,
      "load": ["fill": 0],
    ],
    "scenes": ["materials", "geometry", "lighting"].map {
      [
        "scene": $0, "valid": true, "frames": 1200, "elapsed_seconds": 1200.0 / 144.0,
        "render_fps": 144.0, "errors": [],
      ]
    }, "captures": [],
  ]
}
@Test func internallyInconsistentMeasurementsCannotHaveScores() throws {
  var wrongMean = fixture()
  wrongMean["render_fps"] = 200.0
  var wrongCohort = fixture()
  var scenes = wrongCohort["scenes"] as! [[String: Any]]
  scenes[0]["frames"] = 10
  wrongCohort["scenes"] = scenes
  var invalidMode = fixture()
  var config = invalidMode["config"] as! [String: Any]
  config["scale"] = 0.5
  invalidMode["config"] = config
  var missingChecks = fixture()
  missingChecks.removeValue(forKey: "errors")
  for object in [wrongMean, wrongCohort, invalidMode, missingChecks] {
    let report = try JSONDecoder().decode(
      BenchReport.self, from: JSONSerialization.data(withJSONObject: object))
    #expect(report.score == nil)
  }
}
@Test func failedStoppedAndIncompatibleReportsNeverHaveScores() throws {
  for object in [
    fixture(valid: false), fixture(stopped: true), fixture(schema: 2), fixture(profile: "unknown"),
  ] {
    let report = try JSONDecoder().decode(
      BenchReport.self, from: JSONSerialization.data(withJSONObject: object))
    #expect(report.score == nil)
  }
  let report = try JSONDecoder().decode(
    BenchReport.self, from: JSONSerialization.data(withJSONObject: fixture()))
  #expect(report.score == 144)
  #expect(report.standard)
}
@Test func traversalAndSymlinkEscapesAreRejected() throws {
  let root = try temporaryDirectory()
  defer { try? FileManager.default.removeItem(at: root) }
  #expect(throws: (any Error).self) { try ContainedPath.resolve("../outside.png", in: root) }
  let link = root.appendingPathComponent("link")
  try FileManager.default.createSymbolicLink(
    at: link, withDestinationURL: root.deletingLastPathComponent())
  #expect(throws: (any Error).self) { try ContainedPath.resolve("link/outside.png", in: root) }
}
@Test func exportRejectsEscapedArtifactPathsAndDoesNotLeaveDestination() throws {
  let root = try temporaryDirectory()
  defer { try? FileManager.default.removeItem(at: root) }
  let run = root.appendingPathComponent("run")
  try FileManager.default.createDirectory(at: run, withIntermediateDirectories: true)
  var object = fixture()
  object["captures"] = [["scene": "materials", "tick": 120, "path": "../../outside.png"]]
  let file = run.appendingPathComponent("result.json")
  try JSONSerialization.data(withJSONObject: object).write(to: file)
  let report = try LoadedReport.load(file)
  let store = try RunStore(root: root.appendingPathComponent("history"))
  let destination = root.appendingPathComponent("export")
  #expect(throws: (any Error).self) { try store.export(report, to: destination) }
  #expect(!FileManager.default.fileExists(atPath: destination.path))
}
