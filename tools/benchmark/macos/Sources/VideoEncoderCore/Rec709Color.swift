import Foundation

public enum Rec709Color {
  // sRGB and Rec.709 share D65 and RGB primaries. Decode the sRGB transfer,
  // then apply the Rec.709 OETF; relabeling the original bytes is incorrect.
  private static let table: [UInt8] = (0...255).map { sample in
    let srgb = Double(sample) / 255
    let linear = srgb <= 0.04045 ? srgb / 12.92 : pow((srgb + 0.055) / 1.055, 2.4)
    let rec709 = linear < 0.018 ? 4.5 * linear : 1.099 * pow(linear, 0.45) - 0.099
    return UInt8(clamping: Int((rec709 * 255).rounded()))
  }

  public static func channel(_ value: UInt8) -> UInt8 { table[Int(value)] }

  /// Top-down RGBA sRGB to top-down BGRA Rec.709, retaining row padding.
  public static func convert(
    rgba: Data, width: Int, height: Int, destination: UnsafeMutableRawBufferPointer,
    bytesPerRow: Int
  ) throws {
    guard width > 0, height > 0, width <= VideoHeader.width, height <= VideoHeader.height,
      rgba.count == width * height * 4, bytesPerRow >= width * 4,
      destination.count >= bytesPerRow * height
    else { throw VideoError("Invalid color conversion buffer dimensions") }
    try rgba.withUnsafeBytes { (source: UnsafeRawBufferPointer) in
      for y in 0..<height {
        let inputRow = y * width * 4
        let outputRow = y * bytesPerRow
        for x in 0..<width {
          let input = inputRow + x * 4
          let output = outputRow + x * 4
          guard source[input + 3] == 255 else {
            throw VideoError("Video stream contains non-opaque alpha")
          }
          destination[output] = table[Int(source[input + 2])]
          destination[output + 1] = table[Int(source[input + 1])]
          destination[output + 2] = table[Int(source[input])]
          destination[output + 3] = 255
        }
      }
    }
  }
}
