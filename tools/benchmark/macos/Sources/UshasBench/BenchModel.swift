import AppKit
import BenchCore
import Observation
import SwiftUI

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
  @ObservationIgnored let session = ChildSession()
  @ObservationIgnored var store: RunStore?
  @ObservationIgnored var output: URL?
  @ObservationIgnored var configureTask: Task<Void, Never>?
  @ObservationIgnored var quitAfterStop = false
  @ObservationIgnored var stressPanel: NSPanel?
  init() {
    do {
      let support = try FileManager.default.url(
        for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
      store = try RunStore(
        root: support.appendingPathComponent("Ushas Bench/Runs", isDirectory: true))
      refreshHistory()
    } catch { self.error = "Local history could not be opened: \(error.localizedDescription)" }
    session.onEvent = { [weak self] event in self?.receive(event) }
    session.onFinish = { [weak self] outcome in self?.finish(outcome) }
  }
  var helper: URL { Bundle.main.bundleURL.appendingPathComponent("Contents/Helpers/ushas-bench") }
  func launch(_ command: String) {
    guard !running, let store else { return }
    error = nil
    exportMessage = nil
    selected = nil
    liveFPS = nil
    progress = 0
    activeCommand = command
    let runConfiguration = configuration
    let presentation = RunPresentation(command: command, configuration: runConfiguration)
    activePresentation = presentation
    status = "Opening the render lab…"
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
    }
  }
  func stop() {
    guard running else { return }
    status = "Stopping and preserving the run…"
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
    if let message = event.message {
      status = message
    } else if event.event == "started" {
      status = "Rendering the lab…"
    } else if event.event == "scene_complete" {
      status = "Scene complete. Preparing the next view…"
    } else if event.event == "complete" {
      status = "Checking the result…"
    }
  }
  private func finish(_ outcome: ChildOutcome) {
    let presentation = activePresentation
    running = false
    activePresentation = nil
    configureTask?.cancel()
    configureTask = nil
    stressPanel?.close()
    stressPanel = nil
    if let output {
      do { try store?.record(outcome, output: output) } catch {
        self.error = "The launcher receipt could not be saved: \(error.localizedDescription)"
      }
    }
    selected = outcome.result.map { report in outcome.error.map { report.invalidated($0) } ?? report
    }
    selectedHistoryID = selected?.id
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
  func refreshHistory() { history = store?.history() ?? [] }
  func select(_ item: HistoryEntry) {
    selected = item.result
    selectedHistoryID = item.id
    error = item.error
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
