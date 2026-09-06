import BenchCore
import SwiftUI

struct VideoExportOptions: View {
  @Bindable var model: BenchModel
  private var selectedMode: Binding<String> {
    Binding(
      get: {
        VideoMode(mode: model.videoConfiguration.mode, scale: model.videoConfiguration.scale).id
      },
      set: { id in
        guard let mode = VideoMode.comparison.first(where: { $0.id == id }) else { return }
        model.videoConfiguration.mode = mode.mode
        model.videoConfiguration.scale = mode.scale
      })
  }
  var body: some View {
    VStack(alignment: .leading, spacing: 12) {
      if model.videoComparison {
        Picker("Render mode", selection: selectedMode) {
          ForEach(VideoMode.comparison) { Text($0.title).tag($0.id) }
        }
      } else {
        Text(
          VideoMode(mode: model.videoConfiguration.mode, scale: model.videoConfiguration.scale)
            .title
        )
        .font(.headline)
      }
      Picker("Chapters", selection: $model.videoConfiguration.videoChapter) {
        ForEach(VideoChapter.allCases) { Text($0.title).tag($0) }
      }
      Text("2560 × 1440 · 60 fps · H.264 MP4 · Silent")
      Text(
        model.videoFromSavedResult
          ? "Replay rendered with the current app version using these saved settings. No benchmark score."
          : "A separately rendered replay of the standard lab. No benchmark run required."
      )
      .foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
    }.font(.system(size: 12)).padding(16).frame(width: 420)
  }
}
