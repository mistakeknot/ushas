import Darwin
import Foundation

public struct VideoError: Error, CustomStringConvertible, Sendable {
  public let description: String
  public init(_ description: String) { self.description = description }
}

public struct VideoHeader: Sendable {
  public static let width = 2560
  public static let height = 1440
  public static let fps = 60
  public static let payloadBytes = width * height * 4
  public let frameCount: Int

  public init(data: Data) throws {
    guard data.count == 24, data.prefix(8) == Data("USHASV01".utf8) else {
      throw VideoError("Invalid video stream header or version")
    }
    guard word(data, 8) == Self.width, word(data, 12) == Self.height,
      word(data, 16) == Self.fps, [600, 1800].contains(word(data, 20))
    else { throw VideoError("Video requires 2560 x 1440, 60 fps, and 600 or 1800 frames") }
    frameCount = word(data, 20)
  }
}

public struct VideoFrameMetadata: Sendable {
  public let index: Int
  public let chapter: Int
  public let tick: Int
}

public struct VideoSequence {
  private let header: VideoHeader
  private var nextIndex = 0
  private var firstChapter: Int?

  public init(header: VideoHeader) { self.header = header }

  @discardableResult
  public mutating func accept(metadata: Data) throws -> VideoFrameMetadata {
    guard metadata.count == 16, nextIndex < header.frameCount else {
      throw VideoError("Unexpected video frame after declared sequence")
    }
    let index = word(metadata, 0)
    let chapter = word(metadata, 4)
    let tick = word(metadata, 8)
    guard index == nextIndex, (0...2).contains(chapter), tick == (index % 600) * 2,
      word(metadata, 12) == VideoHeader.payloadBytes
    else {
      throw VideoError(
        "Invalid video frame order, simulation tick, chapter, or payload size at frame \(nextIndex)"
      )
    }
    let expectedChapter = header.frameCount == 1800 ? index / 600 : firstChapter ?? chapter
    guard chapter == expectedChapter else {
      throw VideoError("Unexpected chapter at video frame \(index)")
    }
    firstChapter = firstChapter ?? chapter
    nextIndex += 1
    return VideoFrameMetadata(index: index, chapter: chapter, tick: tick)
  }

  public func finish() throws {
    guard nextIndex == header.frameCount else {
      throw VideoError("Incomplete video sequence: \(nextIndex) of \(header.frameCount) frames")
    }
  }
}

private func word(_ bytes: Data, _ offset: Int) -> Int {
  bytes.withUnsafeBytes {
    Int(UInt32(littleEndian: $0.loadUnaligned(fromByteOffset: offset, as: UInt32.self)))
  }
}

/// Demand-driven admission shared by the real writer and CPU failure tests.
/// The producer cannot advance to another frame while the sink is suspended.
nonisolated(nonsending)
  public func streamVideoFrames(
    header: VideoHeader, reader: ExactVideoReader,
    receive: (VideoFrameMetadata, Data) async throws -> Void
  ) async throws
{
  var sequence = VideoSequence(header: header)
  for _ in 0..<header.frameCount {
    try Task.checkCancellation()
    let frame = try sequence.accept(metadata: reader.readExact(16))
    let rgba = try reader.readExact(VideoHeader.payloadBytes)
    try await receive(frame, rgba)
  }
  try sequence.finish()
  try reader.requireEOF()
}

/// No prefetch and at most one frame of caller-owned storage. Short pipe reads
/// are normal; a 64 KiB read ceiling also bounds transient Data allocations.
public final class ExactVideoReader {
  private let read: (Int) throws -> Data
  public init(read: @escaping (Int) throws -> Data) { self.read = read }

  public func readExact(_ count: Int) throws -> Data {
    guard count >= 0, count <= VideoHeader.payloadBytes else {
      throw VideoError("Video read exceeds one frame")
    }
    var result = Data()
    result.reserveCapacity(count)
    while result.count < count {
      let requested = min(65_536, count - result.count)
      let chunk = try read(requested)
      guard !chunk.isEmpty else {
        throw VideoError(
          "Truncated video stream: expected \(count) bytes, received \(result.count)")
      }
      guard chunk.count <= requested else { throw VideoError("Video source returned excess data") }
      result.append(chunk)
    }
    return result
  }

  public func requireEOF() throws {
    guard try read(1).isEmpty else { throw VideoError("Trailing data after declared video frames") }
  }

  /// Polling keeps cancellation responsive even if the renderer stalls midway
  /// through a frame. Pipe capacity supplies backpressure to the producer.
  public static func standardInput() -> ExactVideoReader {
    ExactVideoReader { count in
      while true {
        try Task.checkCancellation()
        var descriptor = pollfd(fd: STDIN_FILENO, events: Int16(POLLIN), revents: 0)
        let ready = Darwin.poll(&descriptor, 1, 250)
        if ready < 0 {
          if errno == EINTR { continue }
          throw VideoError("Cannot poll video input: \(String(cString: strerror(errno)))")
        }
        if ready == 0 { continue }
        var data = Data(count: count)
        let received = data.withUnsafeMutableBytes {
          Darwin.read(STDIN_FILENO, $0.baseAddress, count)
        }
        if received < 0 {
          if errno == EINTR { continue }
          throw VideoError("Cannot read video input: \(String(cString: strerror(errno)))")
        }
        data.count = received
        return data
      }
    }
  }
}
