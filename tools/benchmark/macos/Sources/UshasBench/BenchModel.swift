import AppKit
import BenchCore
import Observation
import SwiftUI
import UniformTypeIdentifiers

@MainActor @Observable
final class BenchModel {
  enum Page: String, CaseIterable, Identifiable {
    case benchmark = "Benchmark"
    case compare = "Compare"
    case stress = "Stress"
    case results = "Results"
    var id: String { rawValue }
    var symbol: String {
      switch self {
      case .benchmark: "gauge.with.needle"
      case .compare: "rectangle.split.2x1"
      case .stress: "slider.horizontal.3"
      case .results: "tray.full"
      }
    }
  }
  var page: Page = .benchmark
  var configuration = BenchConfiguration()
  var running = false
  var activeCommand = ""
  var activePresentation: RunPresentation?
  var canStopWithEscape: Bool { running && activePresentation?.handlesLauncherEscape == true }
  var progress = 0.0
  var status = "Ready for a fresh run."
  var liveFPS: Double?
  var currentScene = "Preparing the lab"
  var history: [HistoryEntry] = []
  var selected: LoadedReport?
  var selectedHistoryID: String?
  var error: String?
  var exportMessage: String?
  var videoConfiguration = BenchConfiguration()
  var videoComparison = false
  var videoFromSavedResult = false
  var publishedVideo: URL?
  var publishingVideo = false
  @ObservationIgnored var publicationTask: Task<URL, Error>?
  @ObservationIgnored var videoDestination: VideoDestination?
  @ObservationIgnored var videoSavePanel: NSSavePanel?
  @ObservationIgnored let session = ChildSession()
  @ObservationIgnored var store: RunStore?
  @ObservationIgnored var output: URL?
  @ObservationIgnored var configureTask: Task<Void, Never>?
  @ObservationIgnored var quitAfterStop = false
  @ObservationIgnored var stressPanel: NSPanel?
  @ObservationIgnored var statusUpdates = RunStatusUpdates()
  @ObservationIgnored var historyTask: Task<Void, Never>?
  init() {
    do {
      let support = try FileManager.default.url(
        for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
      store = try RunStore(
        root: support.appendingPathComponent("Ushas Bench/Runs", isDirectory: true))
      refreshHistory()
    } catch { self.error = "Local history could not be opened: \(error.localizedDescription)" }
    session.onEvent = { [weak self] event in self?.receive(event) }
    session.onFinish = { [weak self] outcome in Task { await self?.finish(outcome) } }
  }
  var helper: URL { Bundle.main.bundleURL.appendingPathComponent("Contents/Helpers/ushas-bench") }
  func launch(_ command: String, using requested: BenchConfiguration? = nil) {
    guard !running, videoSavePanel == nil, let store else { return }
    error = nil
    exportMessage = nil
    publishedVideo = nil
    selected = nil
    liveFPS = nil
    progress = 0
    activeCommand = command
    statusUpdates = RunStatusUpdates()
    let runConfiguration = requested ?? configuration
    let presentation = RunPresentation(command: command, configuration: runConfiguration)
    activePresentation = presentation
    status = command == "video" ? "Preparing your video…" : "Opening the render lab…"
    currentScene = "Preparing the lab"
    let output = store.newOutput()
    self.output = output
    do {
      try session.start(
        executable: helper, arguments: runConfiguration.arguments(command: command, output: output),
        output: output, logURL: store.logURL(for: output))
      running = true
      switch presentation.launchBehavior {
      case .stayInLauncher:
        break
      case .showStressPanel:
        showStressControls()
        for window in NSApp.windows where window !== stressPanel { window.orderOut(nil) }
        stressPanel?.orderFrontRegardless()
      case .hideLauncher:
        NSApp.hide(nil)
      }
    } catch {
      self.error = error.localizedDescription
      running = false
      activePresentation = nil
      videoDestination = nil
    }
  }
  func stop() {
    guard running else { return }
    statusUpdates.stop()
    status = activeCommand == "video" ? "Cancelling the video…" : "Stopping and preserving the run…"
    if publishingVideo {
      publicationTask?.cancel()
      return
    }
    session.stop()
  }
  func configureStress() {
    guard running, activeCommand == "stress" else { return }
    configureTask?.cancel()
    configureTask = Task { [weak self] in
      try? await Task.sleep(for: .milliseconds(250))
      guard !Task.isCancelled, let self else { return }
      self.session.configure(self.configuration)
    }
  }
  private func receive(_ event: ChildEvent) {
    if let scene = event.scene { currentScene = scene.capitalized }
    if let value = event.progress { progress = value }
    if let value = event.renderFPS { liveFPS = value }
    if let message = statusUpdates.message(
      for: event, command: activeCommand, scene: currentScene)
    {
      status = message
    }
  }
  private func finish(_ outcome: ChildOutcome) async {
    let presentation = activePresentation
    configureTask?.cancel()
    configureTask = nil
    stressPanel?.close()
    stressPanel = nil
    if let output {
      do { try store?.record(outcome, output: output) } catch {
        self.error = "The launcher receipt could not be saved: \(error.localizedDescription)"
      }
    }
    let finishedReport = outcome.result.map { report in
      outcome.error.map { report.invalidated($0) } ?? report
    }
    if activeCommand == "video", !outcome.cancelled, outcome.error == nil,
      let result = finishedReport, let destination = videoDestination
    {
      publishingVideo = true
      status = "Saving your video…"
      let task = Task.detached {
        let source = try result.videoURL()
        try destination.publish(source: source, checkCancellation: { try Task.checkCancellation() })
        return destination.url
      }
      publicationTask = task
      do {
        publishedVideo = try await task.value
        exportMessage = "Video exported."
      } catch is CancellationError {
        exportMessage = "Export cancelled. The completed replay remains in local history."
      } catch {
        self.error =
          "The video is saved in local history, but could not be exported: \(error.localizedDescription)"
      }
      publicationTask = nil
      publishingVideo = false
    }
    videoDestination = nil
    selected = finishedReport
    selectedHistoryID = finishedReport?.id
    running = false
    activePresentation = nil
    status =
      outcome.cancelled
      ? "Stopped. Artifacts saved locally."
      : (outcome.error == nil
        ? "Your result is ready." : "This run could not produce a valid result.")
    if !outcome.cancelled { error = outcome.error ?? error }
    refreshHistory()
    page = .results
    if quitAfterStop {
      NSApp.reply(toApplicationShouldTerminate: true)
    } else if presentation?.activatesResults == true {
      NSApp.unhide(nil)
      NSApp.activate(ignoringOtherApps: true)
      NSApp.windows.first(where: { !($0 is NSPanel) })?.makeKeyAndOrderFront(nil)
    }
  }
  func refreshHistory() {
    historyTask?.cancel()
    guard let store else {
      history = []
      return
    }
    historyTask = Task { [weak self] in
      let entries = await Task.detached { store.history() }.value
      guard !Task.isCancelled else { return }
      self?.history = entries
    }
  }
  func select(_ item: HistoryEntry) {
    selected = item.result
    selectedHistoryID = item.id
    error = item.error
    publishedVideo = nil
    page = .results
  }
  func revealCurrent() {
    if let url = selected?.url ?? output?.appendingPathComponent("result.json") {
      NSWorkspace.shared.activateFileViewerSelecting([url])
    }
  }
  func export() {
    guard let selected, let store else { return }
    let panel = NSSavePanel()
    panel.title = "Export an offline report"
    panel.prompt = "Export"
    panel.nameFieldStringValue =
      "Ushas-\(selected.report.kind)-\(Date.now.formatted(.iso8601.year().month().day().dateSeparator(.dash)))"
    panel.canCreateDirectories = true
    guard panel.runModal() == .OK, let destination = panel.url else { return }
    do {
      try store.export(selected, to: destination)
      exportMessage = "Offline report exported."
      NSWorkspace.shared.activateFileViewerSelecting([destination])
    } catch { self.error = error.localizedDescription }
  }
  func exportVideo(from result: LoadedReport? = nil) {
    guard !running, videoSavePanel == nil else { return }
    var replay = configuration
    videoComparison = result?.report.kind == "compare"
    videoFromSavedResult = result != nil
    if let result {
      guard result.report.standard, ["benchmark", "compare"].contains(result.report.kind) else {
        return
      }
      replay = BenchConfiguration()
      replay.seed = result.report.config?.seed ?? 21434
      if !videoComparison {
        replay.mode = RenderMode(rawValue: result.report.config?.mode ?? "native") ?? .native
        let scale = result.report.config?.scale ?? 1
        replay.scale = abs(scale - 0.5) < 1e-6 ? "1/2" : abs(scale - 2.0 / 3.0) < 1e-6 ? "2/3" : "1"
      }
    }
    replay.background = true
    replay.videoChapter = .all
    videoConfiguration = replay
    let panel = NSSavePanel()
    panel.title = "Export video"
    panel.prompt = "Render video"
    panel.allowedContentTypes = [.mpeg4Movie]
    panel.canCreateDirectories = true
    panel.nameFieldStringValue = "Ushas-\(replay.mode.rawValue).mp4"
    panel.accessoryView = NSHostingView(rootView: VideoExportOptions(model: self))
    videoSavePanel = panel
    // Keep the main actor available for the SwiftUI accessory's menu actions.
    // A nested runModal loop can leave its picker visible but unable to open.
    let completion: (NSApplication.ModalResponse) -> Void = { [weak self] response in
      guard let self else { return }
      self.videoSavePanel = nil
      guard response == .OK, let destination = panel.url else { return }
      do {
        self.videoDestination = try VideoDestination(url: destination)
        self.launch("video", using: self.videoConfiguration)
      } catch { self.error = error.localizedDescription }
    }
    if let window = NSApp.keyWindow {
      panel.beginSheetModal(for: window, completionHandler: completion)
    } else {
      panel.begin(completionHandler: completion)
    }
  }
  func openVideo(reveal: Bool = false) {
    do {
      guard let selected else { return }
      let movie = try publishedVideo ?? selected.videoURL()
      if reveal {
        NSWorkspace.shared.activateFileViewerSelecting([movie])
      } else {
        NSWorkspace.shared.open(movie)
      }
    } catch { self.error = error.localizedDescription }
  }
  func showStressControls() {
    guard running, activeCommand == "stress" else { return }
    if let stressPanel {
      stressPanel.orderFrontRegardless()
      return
    }
    let panel = NSPanel(
      contentRect: NSRect(x: 0, y: 0, width: 352, height: 540),
      styleMask: [.titled, .closable, .utilityWindow, .nonactivatingPanel], backing: .buffered,
      defer: false)
    panel.title = "Ushas · Live controls"
    panel.level = .floating
    panel.isFloatingPanel = true
    panel.hidesOnDeactivate = false
    panel.isReleasedWhenClosed = false
    panel.contentView = NSHostingView(rootView: StressControlPanel(model: self))
    panel.setFrameOrigin(NSPoint(x: 36, y: 100))
    stressPanel = panel
    panel.orderFrontRegardless()
  }
}
