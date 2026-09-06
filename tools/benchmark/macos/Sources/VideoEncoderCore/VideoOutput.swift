import Darwin
import Foundation

/// Owns cleanup from creation through publication. An exclusive lock protects
/// the fixed partial filename; link(2) publishes atomically without replacement.
public final class VideoOutput {
  public let partial: URL
  public let destination: URL
  private let lock: URL
  private var ownsPartial = false

  public init(destination: URL) throws {
    self.destination = destination
    partial = destination.deletingPathExtension().appendingPathExtension("partial.mp4")
    lock = destination.appendingPathExtension("encoding-lock")
    let fd = Darwin.open(lock.path, O_CREAT | O_EXCL | O_WRONLY | O_NOFOLLOW, 0o600)
    guard fd >= 0 else {
      throw VideoError("Cannot reserve video destination: \(String(cString: strerror(errno)))")
    }
    Darwin.close(fd)
    do {
      for path in [destination, partial] {
        var info = stat()
        guard lstat(path.path, &info) != 0, errno == ENOENT else {
          throw VideoError("Video output already exists: \(path.lastPathComponent)")
        }
      }
      ownsPartial = true
    } catch {
      Darwin.unlink(lock.path)
      throw error
    }
  }

  public func publish() throws {
    let fd = Darwin.open(partial.path, O_RDONLY | O_NOFOLLOW)
    guard fd >= 0 else { throw VideoError("Finished video is missing") }
    let syncResult = Darwin.fsync(fd)
    Darwin.close(fd)
    guard syncResult == 0 else { throw VideoError("Cannot flush finished video to disk") }
    guard Darwin.link(partial.path, destination.path) == 0 else {
      throw VideoError(
        "Cannot publish video without replacing an existing file: \(String(cString: strerror(errno)))"
      )
    }
    Darwin.unlink(partial.path)
    ownsPartial = false
  }

  deinit {
    if ownsPartial { Darwin.unlink(partial.path) }
    Darwin.unlink(lock.path)
  }
}
