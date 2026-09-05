import Foundation
import Testing

@testable import BenchCore

private func statusEvent(_ fields: [String: Any]) throws -> ChildEvent {
  var object = fields
  object["schema_version"] = 1
  return try JSONDecoder().decode(
    ChildEvent.self, from: JSONSerialization.data(withJSONObject: object))
}

@Test func measuredProgressReplacesWarmupWithoutRewritingComparisonMessages() throws {
  let warming = try statusEvent([
    "event": "started", "message": "Warming the Claude render lab", "progress": 0,
  ])
  let progress = try statusEvent(["event": "progress", "progress": 0.2])
  let sample = try statusEvent(["event": "progress", "render_fps": 124.0, "valid": true])
  var status = RunStatusUpdates()
  #expect(status.message(for: warming, command: "stress", scene: "Materials") == warming.message)
  #expect(
    status.message(for: progress, command: "stress", scene: "Materials")
      == "Stress running · Materials")
  #expect(
    status.message(for: sample, command: "stress", scene: "Materials")
      == "Stress running · Materials")
  #expect(
    status.message(for: progress, command: "benchmark", scene: "Geometry")
      == "Rendering geometry…")
  let comparison = try statusEvent([
    "event": "progress", "progress": 0.2, "message": "Round 2/4 · Temporal half",
  ])
  #expect(
    status.message(for: comparison, command: "compare", scene: "Materials") == comparison.message)
  #expect(status.message(for: progress, command: "compare", scene: "Materials") == nil)
  for event in [
    try statusEvent(["event": "progress", "progress": 0]),
    try statusEvent(["event": "progress", "render_fps": 124.0, "valid": false]),
  ] {
    #expect(status.message(for: event, command: "stress", scene: "Materials") == nil)
  }
}

@Test func lateEventsPreserveErrorsAndRequestedStop() throws {
  let failure = try statusEvent(["event": "error", "message": "Renderer failed"])
  let lateEvents = [
    try statusEvent(["event": "progress", "progress": 0.4]),
    try statusEvent(["event": "progress", "message": "Late child message"]),
    try statusEvent(["event": "complete"]),
  ]
  var failed = RunStatusUpdates()
  #expect(failed.message(for: failure, command: "stress", scene: "Materials") == failure.message)
  var stopped = RunStatusUpdates()
  stopped.stop()
  for event in lateEvents {
    #expect(failed.message(for: event, command: "stress", scene: "Materials") == nil)
    #expect(stopped.message(for: event, command: "compare", scene: "Materials") == nil)
  }
  #expect(stopped.message(for: failure, command: "compare", scene: "Materials") == nil)
}
