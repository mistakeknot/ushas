import AppKit
import Testing

@testable import BenchCore

private func launcherKey(
  code: UInt16 = 53, modifiers: NSEvent.ModifierFlags = [], repeatKey: Bool = false,
  type: NSEvent.EventType = .keyDown
) throws -> NSEvent {
  try #require(
    NSEvent.keyEvent(
      with: type, location: .zero, modifierFlags: modifiers, timestamp: 0,
      windowNumber: 0, context: nil, characters: "\u{1b}",
      charactersIgnoringModifiers: "\u{1b}", isARepeat: repeatKey, keyCode: code))
}

@Test @MainActor func launcherEscapeConsumesOnlyTheActiveBackgroundRunKey() throws {
  var stops = 0
  let escape = try launcherKey()
  #expect(
    LauncherEscapeRouter.route(
      escape, backgroundRunActive: true, applicationActive: true,
      modalOrTracking: false, stop: { stops += 1 }) == nil)
  #expect(stops == 1)

  for event in [
    try launcherKey(code: 47), try launcherKey(repeatKey: true),
    try launcherKey(type: .keyUp), try launcherKey(modifiers: .command),
    try launcherKey(modifiers: .control), try launcherKey(modifiers: .option),
    try launcherKey(modifiers: .shift),
  ] {
    #expect(
      LauncherEscapeRouter.route(
        event, backgroundRunActive: true, applicationActive: true,
        modalOrTracking: false, stop: { stops += 1 }) === event)
  }
  #expect(stops == 1, "Other keys, modified Escape and repeat events must pass through")
}

@Test @MainActor func launcherEscapePassesThroughInactiveModalAndWindowedContexts() throws {
  let escape = try launcherKey()
  var stops = 0
  for (backgroundRunActive, applicationActive, modalOrTracking) in [
    (false, true, false), (true, false, false), (true, true, true),
  ] {
    #expect(
      LauncherEscapeRouter.route(
        escape, backgroundRunActive: backgroundRunActive, applicationActive: applicationActive,
        modalOrTracking: modalOrTracking, stop: { stops += 1 }) === escape)
  }
  #expect(stops == 0)
}
