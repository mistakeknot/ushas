import SwiftUI

extension Color {
  static let labBackground = Color(red: 0.070, green: 0.088, blue: 0.098)
  static let labSurface = Color(red: 0.105, green: 0.128, blue: 0.137)
  static let labRaised = Color(red: 0.145, green: 0.173, blue: 0.180)
  static let labCoral = Color(red: 0.94, green: 0.55, blue: 0.43)
  static let labInk = Color(red: 0.95, green: 0.93, blue: 0.88)
  static let labMuted = Color(red: 0.57, green: 0.65, blue: 0.65)
  static let labGreen = Color(red: 0.69, green: 0.84, blue: 0.68)
}
struct Eyebrow: View {
  let text: String
  var body: some View {
    Text(text.uppercased()).font(.system(size: 10, weight: .semibold, design: .monospaced))
      .tracking(2).foregroundStyle(Color.labMuted)
  }
}
struct LabButtonStyle: ButtonStyle {
  var prominent = false
  func makeBody(configuration: Configuration) -> some View {
    configuration.label.font(.custom("AvenirNext-DemiBold", size: 14)).padding(.horizontal, 18)
      .padding(.vertical, 12)
      .foregroundStyle(prominent ? Color.labBackground : Color.labInk)
      .background(
        prominent ? Color.labCoral : Color.labRaised, in: RoundedRectangle(cornerRadius: 10)
      )
      .opacity(configuration.isPressed ? 0.7 : 1)
  }
}
struct LabCard<Content: View>: View {
  var padding: CGFloat = 22
  @ViewBuilder let content: Content
  var body: some View {
    content.padding(padding).background(Color.labSurface, in: RoundedRectangle(cornerRadius: 16))
      .overlay(RoundedRectangle(cornerRadius: 16).stroke(Color.white.opacity(0.035)))
  }
}
struct ClaudeMark: View {
  var body: some View {
    Canvas { context, size in
      let side = min(size.width, size.height)
      let center = CGPoint(x: size.width / 2, y: size.height / 2)
      for index in 0..<12 {
        let angle = Double(index) * .pi / 6
        let inner = CGPoint(
          x: center.x + cos(angle) * side * 0.12, y: center.y + sin(angle) * side * 0.12)
        let outer = CGPoint(
          x: center.x + cos(angle) * side * 0.43, y: center.y + sin(angle) * side * 0.43)
        var spoke = Path()
        spoke.move(to: inner)
        spoke.addLine(to: outer)
        context.stroke(
          spoke, with: .color(.labCoral),
          style: StrokeStyle(lineWidth: side * 0.075, lineCap: .round))
      }
      context.fill(
        Path(
          ellipseIn: CGRect(
            x: center.x - side * 0.205, y: center.y - side * 0.225, width: side * 0.41,
            height: side * 0.45)), with: .color(.labInk))
      for x in [-0.075, 0.075] {
        var eye = Path()
        eye.move(to: CGPoint(x: center.x + side * (x - 0.028), y: center.y - side * 0.028))
        eye.addQuadCurve(
          to: CGPoint(x: center.x + side * (x + 0.028), y: center.y - side * 0.028),
          control: CGPoint(x: center.x + side * x, y: center.y - side * 0.155))
        context.stroke(
          eye, with: .color(.labBackground),
          style: StrokeStyle(lineWidth: side * 0.017, lineCap: .round))
      }
      var smile = Path()
      smile.move(to: CGPoint(x: center.x - side * 0.073, y: center.y + side * 0.045))
      smile.addQuadCurve(
        to: CGPoint(x: center.x, y: center.y + side * 0.073),
        control: CGPoint(x: center.x - side * 0.058, y: center.y + side * 0.155))
      smile.addQuadCurve(
        to: CGPoint(x: center.x + side * 0.073, y: center.y + side * 0.045),
        control: CGPoint(x: center.x + side * 0.058, y: center.y + side * 0.155))
      context.stroke(
        smile, with: .color(.labBackground),
        style: StrokeStyle(lineWidth: side * 0.017, lineCap: .round))
    }.accessibilityLabel("Claude by vgel")
  }
}
struct LabGrid: View {
  var body: some View {
    Canvas { context, size in
      var lines = Path()
      for x in stride(from: CGFloat(0), through: size.width, by: 32) {
        lines.move(to: CGPoint(x: x, y: 0))
        lines.addLine(to: CGPoint(x: x, y: size.height))
      }
      for y in stride(from: CGFloat(0), through: size.height, by: 32) {
        lines.move(to: CGPoint(x: 0, y: y))
        lines.addLine(to: CGPoint(x: size.width, y: y))
      }
      context.stroke(lines, with: .color(.labMuted.opacity(0.10)), lineWidth: 0.5)
    }.allowsHitTesting(false)
  }
}
struct StatusPill: View {
  let title: String
  var good = true
  var body: some View {
    HStack(spacing: 6) {
      Circle().fill(good ? Color.labGreen : .labCoral).frame(width: 5, height: 5)
      Text(title)
    }
    .font(.system(size: 10, weight: .medium, design: .monospaced)).foregroundStyle(Color.labInk)
    .padding(.horizontal, 10).padding(.vertical, 7).background(Color.labRaised, in: Capsule())
  }
}
