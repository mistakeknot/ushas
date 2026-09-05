import Darwin
import Foundation

public struct ChildOutcome: Sendable {
  public let result: LoadedReport?
  public let error: String?
  public let cancelled: Bool
  public let exitCode: Int32
}

@MainActor
public final class ChildSession {
  public private(set) var running = false
  public private(set) var cancellationRequested = false
  public var onEvent: (@MainActor @Sendable (ChildEvent) -> Void)?
  public var onFinish: (@MainActor @Sendable (ChildOutcome) -> Void)?
  private var process: Process?
  private var input: FileHandle?
  private var log: FileHandle?
  private var decoder = EventDecoder()
  private var completion: ChildEvent?
  private var eventFailure: String?
  private var expectedReport: URL?
  private var status: Int32?
  private var stdoutEnded = false
  private var stderrEnded = false
  private var runID = UUID()
  private var cancellation: Task<Void, Never>?
  private var readers: [Task<Void, Never>] = []
  private var readHandles: [FileHandle] = []
  private var drainTimeout: Task<Void, Never>?
  public init() {}

  public func start(executable: URL, arguments: [String], output: URL, logURL: URL) throws {
    guard !running else { throw BenchError.invalid("A renderer is already running.") }
    guard !FileManager.default.fileExists(atPath: output.path) else {
      throw BenchError.invalid("A new run needs a fresh output folder.")
    }
    guard FileManager.default.isExecutableFile(atPath: executable.path) else {
      throw BenchError.invalid("The bundled renderer could not be found.")
    }
    guard FileManager.default.createFile(atPath: logURL.path, contents: nil) else {
      throw BenchError.invalid("The run log could not be created.")
    }
    log = try FileHandle(forWritingTo: logURL)
    decoder = EventDecoder()
    completion = nil
    eventFailure = nil
    status = nil
    stdoutEnded = false
    stderrEnded = false
    cancellationRequested = false
    runID = UUID()
    expectedReport = output.appendingPathComponent("result.json").standardizedFileURL
      .resolvingSymlinksInPath()
    Darwin.signal(SIGPIPE, SIG_IGN)
    let child = Process()
    let stdout = Pipe()
    let stderr = Pipe()
    let stdin = Pipe()
    child.executableURL = executable
    child.arguments = arguments
    child.standardOutput = stdout
    child.standardError = stderr
    child.standardInput = stdin
    input = stdin.fileHandleForWriting
    let id = runID
    child.terminationHandler = { [weak self] child in
      let code = child.terminationStatus
      Task { @MainActor [weak self] in
        guard let self, self.runID == id else { return }
        self.status = code
        self.boundOutputDrain(id: id)
        self.finishIfReady()
      }
    }
    let outputStream = Self.stream(stdout.fileHandleForReading)
    let errorStream = Self.stream(stderr.fileHandleForReading)
    process = child
    readHandles = [stdout.fileHandleForReading, stderr.fileHandleForReading]
    running = true
    do { try child.run() } catch {
      running = false
      process = nil
      try? input?.close()
      input = nil
      try? log?.close()
      log = nil
      stdout.fileHandleForReading.readabilityHandler = nil
      stderr.fileHandleForReading.readabilityHandler = nil
      throw error
    }
    readers = [
      Task { [weak self] in
        for await data in outputStream {
          guard let self, self.runID == id else { return }
          self.receive(data)
        }
        guard let self, self.runID == id else { return }
        for event in self.decoder.append(Data(), final: true) { self.receive(event) }
        self.stdoutEnded = true
        self.finishIfReady()
      },
      Task { [weak self] in
        for await data in errorStream {
          guard let self, self.runID == id else { return }
          do { try self.log?.write(contentsOf: data) } catch {
            self.eventFailure = "The renderer log could not be retained."
          }
        }
        guard let self, self.runID == id else { return }
        self.stderrEnded = true
        self.finishIfReady()
      },
    ]
  }
  private nonisolated static func stream(_ handle: FileHandle) -> AsyncStream<Data> {
    AsyncStream { continuation in
      handle.readabilityHandler = { file in
        let data = file.availableData
        if data.isEmpty {
          file.readabilityHandler = nil
          continuation.finish()
        } else {
          continuation.yield(data)
        }
      }
      continuation.onTermination = { _ in
        handle.readabilityHandler = nil
        try? handle.close()
      }
    }
  }
  private func receive(_ data: Data) {
    do { try log?.write(contentsOf: data) } catch {
      eventFailure = "The renderer log could not be retained."
    }
    for event in decoder.append(data) { receive(event) }
  }
  private func receive(_ event: ChildEvent) {
    if event.event == "complete" {
      if completion != nil { eventFailure = "The renderer emitted more than one final result." }
      completion = event
    }
    if event.event == "error" { eventFailure = event.message ?? "The renderer reported an error." }
    onEvent?(event)
  }
  public func configure(_ config: BenchConfiguration) { send(config.stressMessage) }
  private func send(_ data: Data) {
    guard running, !cancellationRequested else { return }
    do { try input?.write(contentsOf: data) } catch {
      eventFailure = "The renderer stopped accepting commands."
    }
  }
  /// Ask the renderer to stop, then terminate and finally kill if needed. The
  /// Process termination handler reaps the child; a new run cannot reuse it.
  public func stop(
    graceNanoseconds: UInt64 = 1_500_000_000, killGraceNanoseconds: UInt64 = 15_000_000_000
  ) {
    guard running, !cancellationRequested else { return }
    send(Data("{\"event\":\"stop\"}\n".utf8))
    cancellationRequested = true
    let id = runID
    cancellation = Task { [weak self] in
      try? await Task.sleep(nanoseconds: graceNanoseconds)
      guard !Task.isCancelled, let self, self.runID == id, self.running else { return }
      if self.process?.isRunning == true { self.process?.terminate() }
      // The comparison coordinator owns renderer process groups. Give it
      // enough time to terminate, kill and reap those children before
      // escalating against the coordinator itself. App termination waits
      // for this session to finish.
      try? await Task.sleep(nanoseconds: killGraceNanoseconds)
      guard !Task.isCancelled, self.runID == id, self.running else { return }
      if let process = self.process, process.isRunning {
        Darwin.kill(process.processIdentifier, SIGKILL)
      }
    }
  }
  private func boundOutputDrain(id: UUID) {
    drainTimeout = Task { [weak self] in
      try? await Task.sleep(for: .seconds(2))
      guard !Task.isCancelled, let self, self.runID == id, self.running else { return }
      if !self.stdoutEnded || !self.stderrEnded {
        self.eventFailure = "The renderer exited without closing its output streams."
        for handle in self.readHandles {
          handle.readabilityHandler = nil
          try? handle.close()
        }
        for reader in self.readers { reader.cancel() }
        self.stdoutEnded = true
        self.stderrEnded = true
        self.finishIfReady()
      }
    }
  }
  private func finishIfReady() {
    guard running, let status, stdoutEnded, stderrEnded else { return }
    running = false
    cancellation?.cancel()
    cancellation = nil
    drainTimeout?.cancel()
    drainTimeout = nil
    try? input?.close()
    input = nil
    try? log?.close()
    log = nil
    var loaded: LoadedReport?
    var failure = eventFailure ?? decoder.errors.first
    if cancellationRequested {
      failure = "Run stopped. Completed artifacts remain in local history."
    }
    if let expectedReport {
      if let complete = completion, let path = complete.report,
        URL(fileURLWithPath: path).standardizedFileURL.resolvingSymlinksInPath() == expectedReport,
        path.hasPrefix("/")
      {
        do { loaded = try LoadedReport.load(expectedReport) } catch {
          failure =
            failure ?? "The renderer result could not be read: \(error.localizedDescription)"
        }
        if complete.valid != loaded?.report.valid {
          failure = failure ?? "The final event and report disagree."
        }
      } else {
        failure = failure ?? "The renderer did not return the expected report."
      }
    }
    if status != 0 {
      failure = failure ?? "The renderer exited with status \(status). Its log is preserved."
    }
    if let loaded, !loaded.accepted { failure = failure ?? loaded.problem ?? loaded.report.failure }
    process?.terminationHandler = nil
    process = nil
    readers.removeAll()
    readHandles.removeAll()
    onFinish?(
      ChildOutcome(
        result: loaded, error: failure, cancelled: cancellationRequested, exitCode: status))
  }
}
