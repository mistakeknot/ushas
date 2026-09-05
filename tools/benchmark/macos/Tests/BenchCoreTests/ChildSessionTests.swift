import Foundation
import Testing

@testable import BenchCore

@MainActor private func runMock(
  script: String, cancel: Bool = false, cancelWhenStarted: Bool = false, arguments: [String] = []
) async throws -> (ChildOutcome, Bool) {
  let root = try temporaryDirectory()
  defer { try? FileManager.default.removeItem(at: root) }
  let output = root.appendingPathComponent("output")
  let executable = root.appendingPathComponent("mock-renderer")
  try ("#!/bin/sh\n" + script).write(to: executable, atomically: true, encoding: .utf8)
  try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: executable.path)
  let session = ChildSession()
  session.onEvent = { event in
    if cancelWhenStarted && event.event == "started" {
      session.stop(graceNanoseconds: 50_000_000, killGraceNanoseconds: 1_000_000_000)
    }
  }
  return try await withCheckedThrowingContinuation { continuation in
    session.onFinish = { outcome in continuation.resume(returning: (outcome, session.running)) }
    do {
      try session.start(
        executable: executable, arguments: [output.path] + arguments, output: output,
        logURL: root.appendingPathComponent("child.log"))
      if cancel { session.stop(graceNanoseconds: 100_000_000, killGraceNanoseconds: 100_000_000) }
    } catch { continuation.resume(throwing: error) }
  }
}
@Test @MainActor func completionMustMatchTheRequestedBackgroundTarget() async throws {
  let json = String(decoding: try JSONSerialization.data(withJSONObject: fixture()), as: UTF8.self)
  let script = """
    mkdir "$1"
    cat > "$1/result.json" <<'REPORT'
    \(json)
    REPORT
    printf '{"schema_version":1,"event":"complete","valid":true,"report":"%s/result.json"}\\n' "$1"
    """
  let (outcome, running) = try await runMock(script: script, arguments: ["--background"])
  #expect(outcome.error?.contains("different execution target") == true)
  #expect(!running)
}
@Test @MainActor func cancellationAllowsCoordinatorCleanupAfterTERM() async throws {
  let script = """
    trap 'sleep 0.3; exit 0' TERM
    printf '%s\\n' '{"schema_version":1,"event":"started"}'
    while read -r line; do :; done
    """
  let (outcome, running) = try await runMock(script: script, cancelWhenStarted: true)
  #expect(outcome.cancelled)
  #expect(outcome.exitCode == 0)
  #expect(!running)
}
@Test @MainActor func zeroExitWithoutCompleteEventIsNotAccepted() async throws {
  let (outcome, running) = try await runMock(
    script: "printf '%s\\n' '{\"schema_version\":1,\"event\":\"started\"}'\nexit 0\n")
  #expect(outcome.exitCode == 0)
  #expect(outcome.error != nil)
  #expect(!running)
}
@Test @MainActor func cancellationKillsAndReapsAnUncooperativeChild() async throws {
  let start = Date()
  let (outcome, running) = try await runMock(
    script: "trap '' TERM\nwhile read -r line; do :; done\n", cancel: true)
  #expect(outcome.cancelled)
  #expect(outcome.error != nil)
  #expect(!running)
  #expect(Date().timeIntervalSince(start) < 4)
}
@Test @MainActor func reportOutsideReservedOutputCannotProduceResult() async throws {
  let (outcome, _) = try await runMock(
    script:
      "printf '%s\\n' '{\"schema_version\":1,\"event\":\"complete\",\"valid\":true,\"report\":\"/tmp/other-result.json\"}'\n"
  )
  #expect(outcome.result == nil)
  #expect(outcome.error != nil)
}
@Test @MainActor func aValidReportArrivingAtProcessExitIsFullyDrained() async throws {
  let json = String(decoding: try JSONSerialization.data(withJSONObject: fixture()), as: UTF8.self)
  let script = """
    mkdir "$1"
    cat > "$1/result.json" <<'REPORT'
    \(json)
    REPORT
    printf '{"schema_version":1,"event":"complete","valid":true,"report":"%s/result.json"}\\n' "$1"
    """
  let (outcome, running) = try await runMock(script: script)
  #expect(outcome.error == nil)
  #expect(outcome.result?.score == 144)
  #expect(!running)
}
@Test func compareArgumentsDoNotOverrideItsFixedArms() {
  let args = BenchConfiguration().arguments(
    command: "compare", output: URL(fileURLWithPath: "/tmp/new-run"))
  #expect(!args.contains("--mode"))
  #expect(!args.contains("--scale"))
  #expect(!args.contains("--duration"))
  #expect(args.contains("--rounds"))
}
@Test func exportKeepsOriginalJSONAndPortableArtifactReferences() throws {
  let root = try temporaryDirectory()
  defer { try? FileManager.default.removeItem(at: root) }
  let run = root.appendingPathComponent("run")
  try FileManager.default.createDirectory(at: run, withIntermediateDirectories: true)
  let image = run.appendingPathComponent("sample.png")
  try Data([1, 2, 3]).write(to: image)
  var object = fixture()
  object["captures"] = [["scene": "materials", "tick": 120, "path": image.path]]
  let original = try JSONSerialization.data(withJSONObject: object)
  let file = run.appendingPathComponent("result.json")
  try original.write(to: file)
  let report = try LoadedReport.load(file)
  let store = try RunStore(root: root.appendingPathComponent("history"))
  let destination = root.appendingPathComponent("export")
  try store.export(report, to: destination)
  #expect(
    try Data(contentsOf: destination.appendingPathComponent("originals/result.json")) == original)
  let portable = try JSONDecoder().decode(
    BenchReport.self, from: Data(contentsOf: destination.appendingPathComponent("result.json")))
  #expect(portable.captures?.first?.path == "sample.png")
  #expect(
    FileManager.default.fileExists(atPath: destination.appendingPathComponent("index.html").path))
}
@Test func historyRetainsProtocolRejectionAndDetectsChangedResults() throws {
  let root = try temporaryDirectory()
  defer { try? FileManager.default.removeItem(at: root) }
  let store = try RunStore(root: root)
  let output = store.newOutput()
  try FileManager.default.createDirectory(at: output, withIntermediateDirectories: false)
  let file = output.appendingPathComponent("result.json")
  try JSONSerialization.data(withJSONObject: fixture()).write(to: file)
  let report = try LoadedReport.load(file)
  try "retained diagnostic".write(to: store.logURL(for: output), atomically: true, encoding: .utf8)
  try store.record(
    ChildOutcome(result: report, error: "Malformed child event", cancelled: false, exitCode: 0),
    output: output)
  #expect(store.history().first?.result?.score == nil)
  #expect(
    FileManager.default.fileExists(atPath: output.appendingPathComponent("launcher.log").path))
  try store.record(
    ChildOutcome(result: report, error: nil, cancelled: false, exitCode: 0), output: output)
  #expect(store.history().first?.result?.score == 144)
  var changed = fixture()
  changed["started_utc"] = "changed"
  try JSONSerialization.data(withJSONObject: changed).write(to: file)
  #expect(store.history().first?.result?.score == nil)
  #expect(store.history().first?.error?.contains("changed") == true)
}
@Test func exportCannotUseASymlinkToCreateOutputInsideItsSource() throws {
  let root = try temporaryDirectory()
  defer { try? FileManager.default.removeItem(at: root) }
  let run = root.appendingPathComponent("run")
  try FileManager.default.createDirectory(at: run, withIntermediateDirectories: false)
  let file = run.appendingPathComponent("result.json")
  try JSONSerialization.data(withJSONObject: fixture()).write(to: file)
  let alias = root.appendingPathComponent("alias")
  try FileManager.default.createSymbolicLink(at: alias, withDestinationURL: run)
  let store = try RunStore(root: root.appendingPathComponent("history"))
  #expect(throws: (any Error).self) {
    try store.export(LoadedReport.load(file), to: alias.appendingPathComponent("export"))
  }
  #expect(!FileManager.default.fileExists(atPath: run.appendingPathComponent("export").path))
}
