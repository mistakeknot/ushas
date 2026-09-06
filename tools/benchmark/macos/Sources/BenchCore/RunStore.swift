import CryptoKit
import Foundation

public struct HistoryEntry: Sendable, Identifiable {
  public let url: URL
  public let date: Date
  public let result: LoadedReport?
  public let error: String?
  public var id: String { url.path }
}
public struct RunStore: Sendable {
  public let root: URL
  public init(root: URL) throws {
    self.root = root.standardizedFileURL.resolvingSymlinksInPath()
    try FileManager.default.createDirectory(at: self.root, withIntermediateDirectories: true)
    try FileManager.default.createDirectory(
      at: self.root.appendingPathComponent("Logs"), withIntermediateDirectories: true)
  }
  public func newOutput() -> URL {
    root.appendingPathComponent(UUID().uuidString.lowercased(), isDirectory: true)
  }
  public func logURL(for output: URL) -> URL {
    root.appendingPathComponent("Logs").appendingPathComponent(output.lastPathComponent + ".log")
  }
  public func record(_ outcome: ChildOutcome, output: URL) throws {
    try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
    let liveLog = logURL(for: output)
    if FileManager.default.fileExists(atPath: liveLog.path) {
      try Data(contentsOf: liveLog).write(
        to: output.appendingPathComponent("launcher.log"), options: .atomic)
    }
    let report = output.appendingPathComponent("result.json")
    let hash = (try? Data(contentsOf: report)).map {
      SHA256.hash(data: $0).map { String(format: "%02x", $0) }.joined()
    }
    let metadata: [String: Any] = [
      "schema_version": 1, "accepted": outcome.error == nil && outcome.result?.accepted == true,
      "error": outcome.error ?? NSNull(), "cancelled": outcome.cancelled,
      "exit_code": outcome.exitCode,
      "report_sha256": hash ?? NSNull(),
    ]
    try JSONSerialization.data(withJSONObject: metadata, options: [.prettyPrinted, .sortedKeys])
      .write(to: output.appendingPathComponent("launcher-validation.json"), options: .atomic)
  }
  public func history() -> [HistoryEntry] {
    let directories =
      (try? FileManager.default.contentsOfDirectory(
        at: root, includingPropertiesForKeys: [.creationDateKey])) ?? []
    return directories.compactMap { directory in
      let resultURL = directory.appendingPathComponent("result.json")
      let validationURL = directory.appendingPathComponent("launcher-validation.json")
      guard directory.lastPathComponent != "Logs",
        FileManager.default.fileExists(atPath: resultURL.path)
          || FileManager.default.fileExists(atPath: validationURL.path)
      else { return nil }
      let validation = (try? Data(contentsOf: validationURL)).flatMap {
        try? JSONSerialization.jsonObject(with: $0) as? [String: Any]
      }
      var validationError =
        validation?["accepted"] as? Bool == false
        ? (validation?["error"] as? String ?? "The launcher could not validate this run.") : nil
      if validation?["accepted"] as? Bool == true {
        let currentHash = (try? Data(contentsOf: resultURL)).map {
          SHA256.hash(data: $0).map { String(format: "%02x", $0) }.joined()
        }
        if currentHash == nil || currentHash != validation?["report_sha256"] as? String {
          validationError = "The saved report changed after the launcher validated it."
        }
      }
      let date =
        (try? directory.resourceValues(forKeys: [.creationDateKey]).creationDate) ?? .distantPast
      do {
        var result = try LoadedReport.load(resultURL)
        if let validationError { result = result.invalidated(validationError) }
        return HistoryEntry(url: resultURL, date: date, result: result, error: validationError)
      } catch {
        return HistoryEntry(
          url: resultURL, date: date, result: nil, error: error.localizedDescription)
      }
    }.sorted { $0.date > $1.date }
  }
  /// Copy a run without following symlinks, and rewrite contained absolute JSON
  /// artifact references. A new offline index has no remote resources.
  public func export(_ report: LoadedReport, to destination: URL) throws {
    let fm = FileManager.default
    let source = try ContainedPath.canonical(report.root)
    let target = try ContainedPath.canonical(destination)
    guard !fm.fileExists(atPath: target.path), target != source,
      !target.path.hasPrefix(source.path + "/")
    else { throw BenchError.invalid("Choose a new export folder outside the original run.") }
    let temporary = target.deletingLastPathComponent().appendingPathComponent(
      ".ushas-export-" + UUID().uuidString)
    do {
      try fm.createDirectory(at: temporary, withIntermediateDirectories: false)
      guard
        let files = fm.enumerator(
          at: source,
          includingPropertiesForKeys: [.isSymbolicLinkKey, .isDirectoryKey, .isRegularFileKey])
      else { throw BenchError.invalid("The run folder could not be read.") }
      for case let file as URL in files {
        let values = try file.resourceValues(forKeys: [
          .isSymbolicLinkKey, .isDirectoryKey, .isRegularFileKey,
        ])
        guard values.isSymbolicLink != true else {
          throw BenchError.invalid("Exports cannot include symbolic links.")
        }
        let relative = String(file.path.dropFirst(source.path.count + 1))
        let copy = temporary.appendingPathComponent(relative)
        if values.isDirectory == true {
          try fm.createDirectory(at: copy, withIntermediateDirectories: true)
        } else if values.isRegularFile == true {
          try fm.copyItem(at: file, to: copy)
          if file.pathExtension.lowercased() == "json" {
            let original = temporary.appendingPathComponent("originals").appendingPathComponent(
              relative)
            try fm.createDirectory(
              at: original.deletingLastPathComponent(), withIntermediateDirectories: true)
            try fm.copyItem(at: file, to: original)
            let object = try JSONSerialization.jsonObject(with: Data(contentsOf: file))
            let rewritten = try rewrite(
              object, source: source, originalParent: file.deletingLastPathComponent())
            try JSONSerialization.data(
              withJSONObject: rewritten, options: [.prettyPrinted, .sortedKeys, .fragmentsAllowed]
            ).write(to: copy)
          }
        } else {
          throw BenchError.invalid("The run contains an unsupported file type.")
        }
      }
      try offlineHTML(report).write(
        to: temporary.appendingPathComponent("index.html"), atomically: true, encoding: .utf8)
      try fm.moveItem(at: temporary, to: target)
    } catch {
      try? fm.removeItem(at: temporary)
      throw error
    }
  }
  private func rewrite(_ value: Any, source: URL, originalParent: URL) throws -> Any {
    if let dictionary = value as? [String: Any] {
      var output: [String: Any] = [:]
      for (key, item) in dictionary {
        if ["path", "report"].contains(key), let path = item as? String {
          let resolved = try ContainedPath.resolve(path, in: source, relativeTo: originalParent)
          let from = try ContainedPath.canonical(originalParent).pathComponents
          let to = resolved.pathComponents
          var shared = 0
          while shared < min(from.count, to.count) && from[shared] == to[shared] { shared += 1 }
          output[key] = (Array(repeating: "..", count: from.count - shared) + to.dropFirst(shared))
            .joined(separator: "/")
        } else {
          output[key] = try rewrite(item, source: source, originalParent: originalParent)
        }
      }
      return output
    }
    if let array = value as? [Any] {
      return try array.map { try rewrite($0, source: source, originalParent: originalParent) }
    }
    return value
  }
  private func offlineHTML(_ report: LoadedReport) -> String {
    func escape(_ text: String) -> String {
      text.replacingOccurrences(of: "&", with: "&amp;").replacingOccurrences(of: "<", with: "&lt;")
        .replacingOccurrences(of: ">", with: "&gt;").replacingOccurrences(of: "\"", with: "&quot;")
    }
    let score =
      report.score.map { String(format: "%.1f", $0) + " completed-render FPS" }
      ?? (report.accepted
        ? report.report.video?.durationLabel ?? "Completed"
        : report.report.kind == "video" ? "Video incomplete" : "No valid score")
    let explanation =
      report.report.kind == "video"
      ? "Separately rendered video replay. No benchmark score."
      : "Completed-render rate. This does not measure GPU busy time or physical panel delivery."
    let movie =
      report.report.kind == "video" && report.accepted
      ? "<p><a href=\"video.mp4\">Open video</a> · 2560 × 1440 · 60 fps · H.264 MP4</p>" : ""
    let arms = report.arms.map {
      "<tr><td>\(escape($0.arm.label)) · Round \($0.arm.round)</td><td>\(report.accepted ? $0.score.map { String(format: "%.1f", $0) } ?? "Unavailable" : "Unavailable")</td></tr>"
    }.joined()
    let captures = (report.report.captures ?? []) + report.arms.flatMap { $0.arm.captures ?? [] }
    let images = captures.compactMap { capture -> String? in
      guard let url = try? report.imageURL(capture) else { return nil }
      guard let root = try? ContainedPath.canonical(report.root) else { return nil }
      let relative = String(url.path.dropFirst(root.path.count + 1))
      return
        "<figure><img src=\"\(escape(relative))\" alt=\"\(escape(capture.scene)) frame \(capture.tick)\"><figcaption>\(escape(capture.scene.capitalized)) · Frame \(capture.tick)</figcaption></figure>"
    }.joined()
    return """
      <!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width"><title>Ushas Bench report</title>
      <style>body{margin:0;background:#151b1e;color:#f6eee5;font:16px/1.6 -apple-system,sans-serif}main{max-width:1100px;margin:60px auto;padding:0 30px}h1{font-size:58px;line-height:1.1;letter-spacing:-2px}h2{color:#ef9278}small,figcaption{color:#a2b1ad}strong{font:44px ui-monospace,monospace;color:#ef9278}table{width:100%;border-collapse:collapse}td{padding:12px;border-bottom:1px solid #374044}img{width:100%;border-radius:14px}figure{margin:34px 0}a{color:#ef9278}</style>
      <main><small>USHAS / RENDER LAB · OFFLINE REPORT</small><h1>Every frame<br>has a character.</h1><strong>\(escape(score))</strong><p>\(escape(report.problem ?? explanation))</p>\(movie)<p>Execution: \(escape(report.report.executionLabel)) · Profile: \(escape(report.report.profileVersion))</p><table>\(arms)</table><h2>Retained images</h2>\(images)<p><a href="result.json">Portable result</a> · <a href="originals/result.json">Original structured result</a> · Source \(escape(report.report.sourceRevision ?? "unavailable"))</p></main></html>
      """
  }
}
