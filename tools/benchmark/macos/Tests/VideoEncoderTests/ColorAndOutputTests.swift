import Foundation
import Testing

@testable import VideoEncoderCore

@Test func transferConversionMatchesSRGBTo709AndPreservesOrientation() throws {
  #expect(Rec709Color.channel(0) == 0)
  #expect(Rec709Color.channel(255) == 255)
  #expect(abs(Int(Rec709Color.channel(128)) - 115) <= 1)
  #expect(abs(Int(Rec709Color.channel(10)) - 3) <= 1)
  for index in 1...255 {
    #expect(Rec709Color.channel(UInt8(index)) >= Rec709Color.channel(UInt8(index - 1)))
  }
  let rgba = Data([255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 128, 128, 128, 255])
  var destination = [UInt8](repeating: 99, count: 24)
  try destination.withUnsafeMutableBytes {
    try Rec709Color.convert(rgba: rgba, width: 2, height: 2, destination: $0, bytesPerRow: 12)
  }
  #expect(Array(destination[0..<8]) == [0, 0, 255, 255, 0, 255, 0, 255])
  #expect(Array(destination[12..<20]) == [255, 0, 0, 255, 115, 115, 115, 255])
  #expect(Array(destination[8..<12]) == [99, 99, 99, 99])
  #expect(throws: (any Error).self) {
    try destination.withUnsafeMutableBytes {
      try Rec709Color.convert(
        rgba: Data([1, 2, 3, 0]), width: 1, height: 1,
        destination: $0, bytesPerRow: 4)
    }
  }
}

func outputFolder() throws -> URL {
  let url = FileManager.default.temporaryDirectory.appendingPathComponent(
    "ushas-video-tests-" + UUID().uuidString)
  try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
  return url
}

@Test func failedEncodingCleansPartialAndPreservesExistingDestination() throws {
  let folder = try outputFolder()
  defer { try? FileManager.default.removeItem(at: folder) }
  let destination = folder.appendingPathComponent("video.mp4")
  let partial = folder.appendingPathComponent("video.partial.mp4")
  do {
    let transaction = try VideoOutput(destination: destination)
    try Data("incomplete".utf8).write(to: transaction.partial)
  }
  #expect(!FileManager.default.fileExists(atPath: destination.path))
  #expect(!FileManager.default.fileExists(atPath: partial.path))
  try Data("existing".utf8).write(to: destination)
  #expect(throws: (any Error).self) { try VideoOutput(destination: destination) }
  #expect(try Data(contentsOf: destination) == Data("existing".utf8))
}

@Test func publicationIsAtomicAndNeverReplacesRacingDestination() throws {
  let folder = try outputFolder()
  defer { try? FileManager.default.removeItem(at: folder) }
  let destination = folder.appendingPathComponent("video.mp4")
  do {
    let transaction = try VideoOutput(destination: destination)
    #expect(throws: (any Error).self) { try VideoOutput(destination: destination) }
    try Data("complete".utf8).write(to: transaction.partial)
    try Data("racer".utf8).write(to: destination)
    #expect(throws: (any Error).self) { try transaction.publish() }
  }
  #expect(try Data(contentsOf: destination) == Data("racer".utf8))
  try FileManager.default.removeItem(at: destination)
  do {
    let transaction = try VideoOutput(destination: destination)
    try Data("complete".utf8).write(to: transaction.partial)
    try transaction.publish()
  }
  #expect(try Data(contentsOf: destination) == Data("complete".utf8))
  #expect(
    !FileManager.default.fileExists(atPath: folder.appendingPathComponent("video.partial.mp4").path)
  )
}
