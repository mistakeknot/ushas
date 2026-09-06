import CryptoKit
import Foundation
import Testing

@testable import BenchCore

func videoFixture(chapter: String? = nil, bytes: Data = Data("movie fixture".utf8)) -> [String: Any]
{
  var object = fixture(profile: "claude-lab-video-v1")
  object["kind"] = "video"
  object["render_fps"] = NSNull()
  var config = object["config"] as! [String: Any]
  config["background"] = true
  config["scene"] = chapter ?? NSNull()
  object["config"] = config
  let chapters: [String] = chapter.map { [$0] } ?? ["materials", "geometry", "lighting"]
  object["scenes"] = chapters.map {
    ["scene": $0, "valid": true, "frames": 1200, "errors": [], "render_fps": NSNull()]
      as [String: Any]
  }
  object["video"] = [
    "path": "video.mp4", "width": 2560, "height": 1440, "fps": 60, "simulation_hz": 120,
    "frame_count": chapter == nil ? 1800 : 600, "duration_seconds": chapter == nil ? 30 : 10,
    "codec": "h264", "bitrate": 30_000_000, "color_space": "rec709",
    "sha256": SHA256.hash(data: bytes).map { String(format: "%02x", $0) }.joined(),
  ]
  return object
}

@Test func videoReportsAreAcceptedWithoutMeasurementScores() throws {
  for chapter in [nil, "materials", "geometry", "lighting"] as [String?] {
    let report = try JSONDecoder().decode(
      BenchReport.self,
      from: JSONSerialization.data(withJSONObject: videoFixture(chapter: chapter)))
    #expect(report.failure == nil)
    #expect(report.score == nil)
  }
}

@Test func videoAlwaysStaysInLauncher() {
  var config = BenchConfiguration()
  config.background = false
  let presentation = RunPresentation(command: "video", configuration: config)
  #expect(presentation.launchBehavior == .stayInLauncher)
  #expect(presentation.handlesLauncherEscape)
}

@Test func invalidVideoCadenceAndScoreCannotBeAccepted() throws {
  for (key, value) in [
    ("fps", 120), ("frame_count", 1799), ("duration_seconds", 29), ("width", 1280),
  ] {
    var object = videoFixture()
    var video = object["video"] as! [String: Any]
    video[key] = value
    object["video"] = video
    let report = try JSONDecoder().decode(
      BenchReport.self,
      from: JSONSerialization.data(withJSONObject: object))
    #expect(report.failure != nil)
  }
  var scored = videoFixture()
  scored["render_fps"] = 60
  let report = try JSONDecoder().decode(
    BenchReport.self,
    from: JSONSerialization.data(withJSONObject: scored))
  #expect(report.failure != nil)
}

@Test func loadedVideoMustMatchItsRetainedFileHash() throws {
  let root = try temporaryDirectory()
  defer { try? FileManager.default.removeItem(at: root) }
  let reportURL = root.appendingPathComponent("result.json")
  try JSONSerialization.data(withJSONObject: videoFixture()).write(to: reportURL)
  try Data("movie fixture".utf8).write(to: root.appendingPathComponent("video.mp4"))
  #expect(try LoadedReport.load(reportURL).accepted)
  try Data("changed".utf8).write(to: root.appendingPathComponent("video.mp4"))
  #expect(try !LoadedReport.load(reportURL).accepted)
}

@Test func moviePublicationPreservesUnapprovedAndChangedDestinations() throws {
  let root = try temporaryDirectory()
  defer { try? FileManager.default.removeItem(at: root) }
  let source = root.appendingPathComponent("source.mp4")
  let destination = root.appendingPathComponent("destination.mp4")
  try Data("new movie".utf8).write(to: source)
  let emptyDestination = try VideoDestination(url: destination)
  try Data("appeared while rendering".utf8).write(to: destination)
  #expect(throws: (any Error).self) { try emptyDestination.publish(source: source) }
  #expect(try Data(contentsOf: destination) == Data("appeared while rendering".utf8))
  let approvedReplacement = try VideoDestination(url: destination)
  try approvedReplacement.publish(source: source)
  #expect(try Data(contentsOf: destination) == Data("new movie".utf8))
  #expect(FileManager.default.fileExists(atPath: source.path))
  let changedDestination = try VideoDestination(url: destination)
  try Data("changed since Save".utf8).write(to: destination)
  #expect(throws: (any Error).self) { try changedDestination.publish(source: source) }
  #expect(try Data(contentsOf: destination) == Data("changed since Save".utf8))
  #expect(
    try FileManager.default.contentsOfDirectory(atPath: root.path).sorted() == [
      "destination.mp4", "source.mp4",
    ])
}

@Test func moviePublicationFailureLeavesExistingFileAndNoTemporaryData() throws {
  let root = try temporaryDirectory()
  defer { try? FileManager.default.removeItem(at: root) }
  let destination = root.appendingPathComponent("movie.mp4")
  try Data("old movie".utf8).write(to: destination)
  let approved = try VideoDestination(url: destination)
  #expect(throws: (any Error).self) {
    try approved.publish(source: root.appendingPathComponent("missing.mp4"))
  }
  #expect(try Data(contentsOf: destination) == Data("old movie".utf8))
  #expect(try FileManager.default.contentsOfDirectory(atPath: root.path) == ["movie.mp4"])
}

@Test func videoArgumentsPreserveChapterSeedAndModeWithoutStressControls() {
  var config = BenchConfiguration()
  config.background = false
  config.videoChapter = .geometry
  config.mode = .temporal
  config.scale = "2/3"
  config.seed = 42
  config.fill = 5
  let args = config.arguments(command: "video", output: URL(fileURLWithPath: "/tmp/movie"))
  #expect(args.contains("--background"))
  #expect(args[args.firstIndex(of: "--scene")! + 1] == "geometry")
  #expect(args[args.firstIndex(of: "--seed")! + 1] == "42")
  #expect(!args.contains("--fill"))
}

@Test func cancellingPublicationRetainsDestinationAndRemovesTemporaryCopy() throws {
  let root = try temporaryDirectory()
  defer { try? FileManager.default.removeItem(at: root) }
  let destination = root.appendingPathComponent("movie.mp4")
  let source = root.appendingPathComponent("source.mp4")
  try Data("existing".utf8).write(to: destination)
  try Data(repeating: 123, count: 3_000_000).write(to: source)
  let approved = try VideoDestination(url: destination)
  var calls = 0
  #expect(throws: CancellationError.self) {
    try approved.publish(
      source: source,
      checkCancellation: {
        calls += 1
        if calls > 1 { throw CancellationError() }
      })
  }
  #expect(calls == 2)
  #expect(try Data(contentsOf: destination) == Data("existing".utf8))
  #expect(
    try FileManager.default.contentsOfDirectory(atPath: root.path).sorted() == [
      "movie.mp4", "source.mp4",
    ])
}
