import Foundation
import Testing

@testable import VideoEncoderCore

func words(_ values: [UInt32]) -> Data {
  var bytes = Data()
  for value in values {
    var little = value.littleEndian
    withUnsafeBytes(of: &little) { bytes.append(contentsOf: $0) }
  }
  return bytes
}

func streamHeader(_ count: UInt32 = 600) -> Data {
  Data("USHASV01".utf8) + words([2560, 1440, 60, count])
}

@Test func headerRejectsMalformedContract() throws {
  #expect(try VideoHeader(data: streamHeader()).frameCount == 600)
  #expect(try VideoHeader(data: streamHeader(1800)).frameCount == 1800)
  for bytes in [
    Data(), streamHeader().dropLast(), Data("USHASV02".utf8) + words([2560, 1440, 60, 600]),
    Data("USHASV01".utf8) + words([1920, 1440, 60, 600]),
    Data("USHASV01".utf8) + words([2560, 1440, 120, 600]), streamHeader(1200),
  ] {
    #expect(throws: (any Error).self) { try VideoHeader(data: bytes) }
  }
}

@Test func frameSequenceEnforcesCadenceChaptersAndCompletion() throws {
  for (count, firstChapter) in [(600, 0), (600, 1), (600, 2), (1800, 0)] {
    var sequence = VideoSequence(header: try VideoHeader(data: streamHeader(UInt32(count))))
    for index in 0..<count {
      let chapter = count == 1800 ? index / 600 : firstChapter
      let metadata = words([UInt32(index), UInt32(chapter), UInt32(index % 600 * 2), 14_745_600])
      let frame = try sequence.accept(metadata: metadata)
      #expect(frame.index == index)
      #expect(frame.tick == index % 600 * 2)
    }
    try sequence.finish()
    #expect(throws: (any Error).self) {
      try sequence.accept(metadata: words([UInt32(count), 2, 0, 14_745_600]))
    }
  }
  for invalid in [
    words([1, 0, 0, 14_745_600]), words([0, 3, 0, 14_745_600]),
    words([0, 0, 1, 14_745_600]), words([0, 0, 0, 4]),
  ] {
    var sequence = VideoSequence(header: try VideoHeader(data: streamHeader()))
    #expect(throws: (any Error).self) { try sequence.accept(metadata: invalid) }
  }
  var full = VideoSequence(header: try VideoHeader(data: streamHeader(1800)))
  #expect(throws: (any Error).self) {
    try full.accept(metadata: words([0, 1, 0, 14_745_600]))
  }
  var single = VideoSequence(header: try VideoHeader(data: streamHeader()))
  _ = try single.accept(metadata: words([0, 1, 0, 14_745_600]))
  #expect(throws: (any Error).self) {
    try single.accept(metadata: words([1, 2, 2, 14_745_600]))
  }
  #expect(throws: (any Error).self) { try single.finish() }
}

@Test func exactReaderHandlesFragmentationTruncationAndTrailingBytes() throws {
  var source = streamHeader() + words([0, 0, 0, 14_745_600]) + Data([0, 0, 0, 255])
  var maximumRequest = 0
  let reader = ExactVideoReader { requested in
    maximumRequest = max(maximumRequest, requested)
    let chunk = Data(source.prefix(min(requested, 3)))
    source.removeFirst(chunk.count)
    return chunk
  }
  #expect(try reader.readExact(24) == streamHeader())
  _ = try reader.readExact(16)
  #expect(throws: (any Error).self) { try reader.requireEOF() }
  #expect(throws: (any Error).self) { try reader.readExact(14_745_600) }
  #expect(maximumRequest <= 65_536)
  try reader.requireEOF()
}

@Test func sourceErrorsPropagateWithoutAnotherRead() throws {
  var calls = 0
  let reader = ExactVideoReader { _ in
    calls += 1
    throw VideoError("source failed")
  }
  #expect(throws: (any Error).self) { try reader.readExact(24) }
  #expect(calls == 1)
}

@Test func sinkFailureStopsAdmissionAndCleansPartialVideo() async throws {
  let folder = try outputFolder()
  defer { try? FileManager.default.removeItem(at: folder) }
  let destination = folder.appendingPathComponent("video.mp4")
  let header = try VideoHeader(data: streamHeader())
  var source = words([0, 0, 0, 14_745_600]) + Data(repeating: 255, count: VideoHeader.payloadBytes)
  var readBytes = 0
  let reader = ExactVideoReader { requested in
    let result = Data(source.prefix(requested))
    source.removeFirst(result.count)
    readBytes += result.count
    return result
  }
  do {
    let output = try VideoOutput(destination: destination)
    try Data("partial".utf8).write(to: output.partial)
    try await streamVideoFrames(header: header, reader: reader) { frame, rgba in
      #expect(frame.index == 0)
      #expect(rgba.count == VideoHeader.payloadBytes)
      #expect(readBytes == 16 + VideoHeader.payloadBytes)
      throw VideoError("encoder failed")
    }
    Issue.record("Sink failure must propagate")
  } catch let error as VideoError {
    #expect(error.description == "encoder failed")
  }
  #expect(readBytes == 16 + VideoHeader.payloadBytes)
  #expect(!FileManager.default.fileExists(atPath: destination.path))
  #expect(
    !FileManager.default.fileExists(atPath: folder.appendingPathComponent("video.partial.mp4").path)
  )
}

@Test func cancelledReplayReadsNoFrames() async throws {
  let task = Task {
    withUnsafeCurrentTask { $0?.cancel() }
    let reader = ExactVideoReader { _ in
      Issue.record("Cancelled replay must not read another frame")
      return Data()
    }
    try await streamVideoFrames(header: VideoHeader(data: streamHeader()), reader: reader) { _, _ in
      Issue.record("Cancelled replay must not admit another frame")
    }
  }
  do {
    try await task.value
    Issue.record("Cancellation must propagate")
  } catch {
    #expect(error is CancellationError)
  }
}
