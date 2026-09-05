import AppKit

// Procedural mark, rendered locally without downloads or external artwork.
let directory = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
for (points, scale) in [
  (16, 1), (16, 2), (32, 1), (32, 2), (128, 1), (128, 2), (256, 1), (256, 2), (512, 1), (512, 2),
] {
  let pixels = points * scale
  let bitmap = NSBitmapImageRep(
    bitmapDataPlanes: nil, pixelsWide: pixels, pixelsHigh: pixels,
    bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
    colorSpaceName: .deviceRGB,
    bytesPerRow: 0, bitsPerPixel: 0)!
  NSGraphicsContext.saveGraphicsState()
  NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: bitmap)
  let transform = NSAffineTransform()
  transform.scale(by: CGFloat(pixels) / 1024)
  transform.concat()
  NSColor(calibratedRed: 0.075, green: 0.095, blue: 0.10, alpha: 1).setFill()
  NSBezierPath(
    roundedRect: NSRect(x: 56, y: 56, width: 912, height: 912), xRadius: 205, yRadius: 205
  ).fill()
  NSColor(calibratedRed: 0.93, green: 0.53, blue: 0.40, alpha: 1).setStroke()
  for ray in 0..<12 {
    let angle = Double(ray) * .pi / 6
    let path = NSBezierPath()
    path.lineWidth = 75
    path.lineCapStyle = .round
    path.move(to: NSPoint(x: 512 + cos(angle) * 166, y: 516 + sin(angle) * 166))
    path.line(to: NSPoint(x: 512 + cos(angle) * 313, y: 516 + sin(angle) * 313))
    path.stroke()
  }
  NSColor(calibratedRed: 0.99, green: 0.91, blue: 0.80, alpha: 1).setFill()
  NSBezierPath(ovalIn: NSRect(x: 306, y: 310, width: 412, height: 412)).fill()
  NSColor(calibratedRed: 0.13, green: 0.16, blue: 0.16, alpha: 1).setFill()
  NSBezierPath(ovalIn: NSRect(x: 405, y: 535, width: 35, height: 48)).fill()
  NSBezierPath(ovalIn: NSRect(x: 584, y: 535, width: 35, height: 48)).fill()
  NSColor(calibratedRed: 0.13, green: 0.16, blue: 0.16, alpha: 1).setStroke()
  let smile = NSBezierPath()
  smile.lineWidth = 20
  smile.lineCapStyle = .round
  smile.lineJoinStyle = .round
  smile.move(to: NSPoint(x: 455, y: 465))
  smile.line(to: NSPoint(x: 484, y: 435))
  smile.line(to: NSPoint(x: 512, y: 459))
  smile.line(to: NSPoint(x: 540, y: 435))
  smile.line(to: NSPoint(x: 569, y: 465))
  smile.stroke()
  NSGraphicsContext.restoreGraphicsState()
  let suffix = scale == 2 ? "@2x" : ""
  try bitmap.representation(using: .png, properties: [:])!.write(
    to: directory.appendingPathComponent("icon_\(points)x\(points)\(suffix).png"))
}
