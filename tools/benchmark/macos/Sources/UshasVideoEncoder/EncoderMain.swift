import AVFoundation
import CoreVideo
import Darwin
import Foundation
import VideoEncoderCore

@main
struct EncoderMain {
  static func main() async {
    let arguments = CommandLine.arguments
    guard arguments.count == 3, arguments[1] == "--out", arguments[2].hasSuffix(".mp4") else {
      diagnostic("Usage: ushas-video-encoder --out /new/run/video.mp4")
      exit(2)
    }
    let output = URL(fileURLWithPath: arguments[2])
    let job = Task.detached { try await encode(to: output) }
    // Dispatch signal handlers cancel the async admission and the interruptible
    // input reader; cleanup stays in ordinary Swift control flow.
    signal(SIGTERM, SIG_IGN)
    signal(SIGINT, SIG_IGN)
    let signals = [SIGTERM, SIGINT].map { value in
      let source = DispatchSource.makeSignalSource(signal: value, queue: .global())
      source.setEventHandler { @Sendable in job.cancel() }
      source.resume()
      return source
    }
    do {
      try await job.value
      for source in signals { source.cancel() }
      diagnostic("Video encoding complete: \(output.lastPathComponent)")
    } catch {
      for source in signals { source.cancel() }
      diagnostic("Video encoding failed: \(error)")
      exit(error is CancellationError ? 130 : 1)
    }
  }

  @MainActor
  static func encode(to destination: URL) async throws {
    let reader = ExactVideoReader.standardInput()
    let header = try VideoHeader(data: reader.readExact(24))
    let output = try VideoOutput(destination: destination)
    let writer = try AVAssetWriter(outputURL: output.partial, fileType: .mp4)
    let cancellation = WriterCancellation(writer: writer)
    let input = AVAssetWriterInput(
      mediaType: .video,
      outputSettings: [
        AVVideoCodecKey: AVVideoCodecType.h264,
        AVVideoWidthKey: VideoHeader.width,
        AVVideoHeightKey: VideoHeader.height,
        AVVideoColorPropertiesKey: [
          AVVideoColorPrimariesKey: AVVideoColorPrimaries_ITU_R_709_2,
          AVVideoTransferFunctionKey: AVVideoTransferFunction_ITU_R_709_2,
          AVVideoYCbCrMatrixKey: AVVideoYCbCrMatrix_ITU_R_709_2,
        ],
        AVVideoCompressionPropertiesKey: [
          AVVideoAverageBitRateKey: 30_000_000,
          AVVideoExpectedSourceFrameRateKey: VideoHeader.fps,
          AVVideoMaxKeyFrameIntervalKey: VideoHeader.fps,
          AVVideoAllowFrameReorderingKey: false,
          AVVideoProfileLevelKey: AVVideoProfileLevelH264HighAutoLevel,
        ],
      ])
    input.mediaTimeScale = Int32(VideoHeader.fps)
    let attributes = CVPixelBufferCreationAttributes(
      pixelFormatType: CVPixelFormatType(rawValue: kCVPixelFormatType_32BGRA),
      size: CVImageSize(width: VideoHeader.width, height: VideoHeader.height))
    guard writer.canAdd(input) else {
      throw VideoError("H.264 encoder cannot accept the video input")
    }
    // Creating a receiver adds its input to the writer.
    let receiver = writer.inputPixelBufferReceiver(for: input, pixelBufferAttributes: attributes)
    try writer.start()
    writer.startSession(atSourceTime: .zero)
    do {
      try await withTaskCancellationHandler {
        guard let pool = receiver.pixelBufferPool else {
          throw VideoError("Video encoder did not create a pixel buffer pool")
        }
        try await streamVideoFrames(header: header, reader: reader) { frame, rgba in
          let mutable = try pool.makeMutablePixelBuffer()
          try mutable.withUnsafeBuffer { pixelBuffer in
            let result = CVPixelBufferLockBaseAddress(pixelBuffer, [])
            guard result == kCVReturnSuccess else {
              throw VideoError("Cannot lock encoder pixel buffer: \(result)")
            }
            defer { CVPixelBufferUnlockBaseAddress(pixelBuffer, []) }
            guard let base = CVPixelBufferGetBaseAddress(pixelBuffer) else {
              throw VideoError("Encoder pixel buffer has no storage")
            }
            let stride = CVPixelBufferGetBytesPerRow(pixelBuffer)
            try Rec709Color.convert(
              rgba: rgba, width: VideoHeader.width, height: VideoHeader.height,
              destination: UnsafeMutableRawBufferPointer(
                start: base, count: stride * VideoHeader.height), bytesPerRow: stride)
            for (key, value) in [
              (kCVImageBufferColorPrimariesKey, kCVImageBufferColorPrimaries_ITU_R_709_2),
              (kCVImageBufferTransferFunctionKey, kCVImageBufferTransferFunction_ITU_R_709_2),
              (kCVImageBufferYCbCrMatrixKey, kCVImageBufferYCbCrMatrix_ITU_R_709_2),
            ] { CVBufferSetAttachment(pixelBuffer, key, value, .shouldPropagate) }
          }
          let pixelBuffer = CVReadOnlyPixelBuffer(mutable)
          // This await is the sole admission point. Do not read or allocate the
          // next frame until the receiver admits this one; never drop frames.
          try await receiver.append(
            pixelBuffer, with: CMTime(value: Int64(frame.index), timescale: Int32(VideoHeader.fps)))
        }
        try Task.checkCancellation()
        writer.endSession(
          atSourceTime: CMTime(value: Int64(header.frameCount), timescale: Int32(VideoHeader.fps)))
        receiver.finish()
        await withCheckedContinuation { continuation in
          writer.finishWriting { continuation.resume() }
        }
        try Task.checkCancellation()
        guard writer.status == .completed else {
          throw writer.error ?? VideoError("Video writer did not complete")
        }
        try output.publish()
      } onCancel: {
        Task { @MainActor in cancellation.cancel() }
      }
    } catch {
      writer.cancelWriting()
      throw error
    }
  }

  static func diagnostic(_ message: String) {
    try? FileHandle.standardError.write(contentsOf: Data((message + "\n").utf8))
  }
}

/// Keep AVAssetWriter access on the same actor, including cancellation while
/// the receiver's asynchronous admission is suspended.
@MainActor
private final class WriterCancellation {
  let writer: AVAssetWriter
  init(writer: AVAssetWriter) { self.writer = writer }
  func cancel() { writer.cancelWriting() }
}
