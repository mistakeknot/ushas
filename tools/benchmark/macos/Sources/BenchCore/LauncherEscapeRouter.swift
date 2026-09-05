import AppKit

@MainActor public enum LauncherEscapeRouter {
  /// Return nil only when this launcher consumes Escape to stop its own run.
  public static func route(
    _ event: NSEvent, backgroundRunActive: Bool, applicationActive: Bool,
    modalOrTracking: Bool, stop: () -> Void
  ) -> NSEvent? {
    guard event.type == .keyDown, event.keyCode == 53, !event.isARepeat,
      event.modifierFlags.intersection([.command, .control, .option, .shift]).isEmpty,
      backgroundRunActive, applicationActive, !modalOrTracking
    else { return event }
    stop()
    return nil
  }
}
