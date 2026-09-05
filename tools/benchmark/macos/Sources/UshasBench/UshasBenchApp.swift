import AppKit
import BenchCore
import SwiftUI

@MainActor final class BenchApplicationDelegate: NSObject, NSApplicationDelegate {
  weak var model: BenchModel?
  private var escapeMonitor: Any?
  func applicationDidFinishLaunching(_ notification: Notification) {
    // Local events only: nested menu/control tracking retains its normal Escape.
    escapeMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
      let consumed = MainActor.assumeIsolated {
        LauncherEscapeRouter.route(
          event, backgroundRunActive: self?.model?.canStopWithEscape == true,
          applicationActive: NSApp.isActive,
          modalOrTracking: NSApp.modalWindow != nil
            || NSApp.windows.contains { $0.attachedSheet != nil || $0.sheetParent != nil }
            || RunLoop.current.currentMode == .eventTracking,
          stop: { self?.model?.stop() }) == nil
      }
      return consumed ? nil : event
    }
  }
  func applicationWillTerminate(_ notification: Notification) {
    if let escapeMonitor { NSEvent.removeMonitor(escapeMonitor) }
    escapeMonitor = nil
  }
  func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
    guard let model, model.running else { return .terminateNow }
    model.quitAfterStop = true
    model.stop()
    return .terminateLater
  }
  func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }
  func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool
  {
    if !flag { sender.windows.first(where: { !($0 is NSPanel) })?.makeKeyAndOrderFront(nil) }
    return true
  }
}
@main struct UshasBenchApp: App {
  @NSApplicationDelegateAdaptor(BenchApplicationDelegate.self) var delegate
  @State private var model = BenchModel()
  var body: some Scene {
    WindowGroup("Ushas Bench") {
      ContentView(model: model).onAppear { delegate.model = model }.preferredColorScheme(.dark)
    }
    .defaultSize(width: 1180, height: 800)
    .windowStyle(.hiddenTitleBar)
    .commands {
      CommandGroup(after: .newItem) {
        Button("Run benchmark") { model.launch("benchmark") }.keyboardShortcut("r").disabled(
          model.running)
        Button("Stop current run") { model.stop() }.keyboardShortcut(".").disabled(!model.running)
        Button("Stop background run") { model.stop() }
          .disabled(!model.canStopWithEscape)
        Button("Show live stress controls") { model.showStressControls() }.disabled(
          !model.running || model.activeCommand != "stress")
      }
    }
  }
}
