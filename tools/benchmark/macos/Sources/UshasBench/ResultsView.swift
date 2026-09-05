import BenchCore
import SwiftUI

struct ResultsView: View {
  @Bindable var model: BenchModel
  var body: some View {
    VStack(alignment: .leading, spacing: 20) {
      if let result = model.selected {
        HStack(alignment: .top, spacing: 20) {
          LabCard {
            VStack(alignment: .leading, spacing: 13) {
              Eyebrow(
                text: result.report.kind == "compare"
                  ? "Comparison result" : "Completed-render throughput")
              if let score = result.score {
                HStack(alignment: .firstTextBaseline, spacing: 9) {
                  Text(String(format: "%.1f", score)).font(
                    .system(size: 63, weight: .light, design: .monospaced)
                  ).tracking(-3).foregroundStyle(Color.labCoral)
                  Text("FPS").font(.system(size: 13, design: .monospaced)).foregroundStyle(
                    Color.labMuted)
                }
              } else {
                Text(result.accepted ? "Completed" : "No valid score").font(
                  .custom("AvenirNext-DemiBold", size: 32)
                ).foregroundStyle(result.accepted ? Color.labInk : .labCoral)
              }
              Text(
                result.problem ?? result.report.failure
                  ?? "The measured rendering cohort, through its closing queue completion. This is not displayed FPS or GPU-only time."
              )
              .font(.system(size: 12)).foregroundStyle(Color.labMuted).fixedSize(
                horizontal: false, vertical: true)
              HStack {
                StatusPill(
                  title: result.accepted
                    ? (result.report.standard ? "STANDARD PROFILE" : "CUSTOM") : "UNQUALIFIED",
                  good: result.accepted)
                StatusPill(
                  title: result.accepted ? "VALID" : "NOT QUALIFIED", good: result.accepted)
              }
            }.frame(maxWidth: .infinity, alignment: .leading)
          }
          VStack(alignment: .leading, spacing: 12) {
            Button {
              model.export()
            } label: {
              Label("Export offline report", systemImage: "square.and.arrow.up")
            }.buttonStyle(LabButtonStyle(prominent: true))
            Button {
              model.revealCurrent()
            } label: {
              Label("Show saved files", systemImage: "folder")
            }.buttonStyle(LabButtonStyle())
            if let message = model.exportMessage {
              Text(message).font(.system(size: 11)).foregroundStyle(Color.labGreen)
            }
            Text(result.report.startedUTC ?? "Saved locally").font(
              .system(size: 10, design: .monospaced)
            ).foregroundStyle(Color.labMuted)
          }.frame(width: 207, alignment: .leading)
        }
        if let scenes = result.report.scenes, !scenes.isEmpty {
          HStack(spacing: 13) {
            ForEach(scenes) { scene in
              LabCard {
                VStack(alignment: .leading, spacing: 10) {
                  Eyebrow(text: scene.scene)
                  Text(
                    result.accepted && scene.valid
                      ? scene.renderFPS.map { String(format: "%.1f", $0) } ?? "—" : "—"
                  ).font(.system(size: 28, weight: .light, design: .monospaced))
                  Text("completed-render FPS").font(.system(size: 10)).foregroundStyle(
                    Color.labMuted)
                }.frame(maxWidth: .infinity, alignment: .leading)
              }
            }
          }
        }
        if !result.arms.isEmpty {
          LabCard {
            VStack(alignment: .leading, spacing: 15) {
              Eyebrow(text: "Every arm, including failures")
              ForEach(result.arms) { arm in
                HStack {
                  Text(arm.arm.label).font(.custom("AvenirNext-DemiBold", size: 13))
                  Text("Round \(arm.arm.round)").font(.system(size: 10, design: .monospaced))
                    .foregroundStyle(Color.labMuted)
                  Spacer()
                  Text(
                    result.accepted
                      ? arm.score.map { String(format: "%.1f FPS", $0) } ?? "Unavailable"
                      : "Unavailable"
                  )
                  .font(.system(size: 15, design: .monospaced)).foregroundStyle(
                    arm.problem == nil && result.accepted ? Color.labCoral : .labMuted)
                }
                if let issue = arm.problem {
                  Text(issue).font(.system(size: 10)).foregroundStyle(Color.labMuted)
                }
                Rectangle().fill(Color.white.opacity(0.06)).frame(height: 1)
              }
            }
          }
        }
        if result.accepted, let summaries = result.report.pairedSummaries, !summaries.isEmpty {
          LabCard {
            VStack(alignment: .leading, spacing: 16) {
              Eyebrow(text: "Paired render-time comparison")
              ForEach(summaries) { summary in
                HStack(alignment: .top) {
                  VStack(alignment: .leading, spacing: 5) {
                    Text(summary.label).font(.custom("AvenirNext-DemiBold", size: 13))
                    Text(summary.performanceGate ?? "No performance conclusion").font(
                      .system(size: 11)
                    ).foregroundStyle(Color.labMuted)
                  }
                  Spacer()
                  VStack(alignment: .trailing, spacing: 5) {
                    Text(
                      summary.valid
                        ? summary.timeReduction.map {
                          String(format: "%+.1f%% render-time reduction", $0 * 100)
                        } ?? "Unavailable" : "Unavailable"
                    )
                    .font(.system(size: 12, design: .monospaced)).foregroundStyle(Color.labCoral)
                    if summary.qualified == true, let interval = summary.ci95, interval.count == 2 {
                      Text(
                        String(
                          format: "95%% interval %+.1f%% to %+.1f%%", interval[0] * 100,
                          interval[1] * 100)
                      ).font(.system(size: 10, design: .monospaced)).foregroundStyle(Color.labMuted)
                    }
                  }
                }
              }
              Text(
                "Timing does not qualify image quality. Inspect the retained faces, thin edges and motion before choosing a reconstruction mode."
              )
              .font(.system(size: 11)).foregroundStyle(Color.labMuted)
            }
          }
        }
        CaptureComparison(result: result)
      } else if model.history.isEmpty {
        LabCard {
          VStack(spacing: 18) {
            ClaudeMark().frame(width: 115, height: 115)
            Text("Your first result starts here.").font(.custom("AvenirNext-DemiBold", size: 27))
            Text(
              "Run a benchmark or compare the six rendering modes. Every attempt is kept locally."
            ).font(.custom("AvenirNext-Regular", size: 13)).foregroundStyle(Color.labMuted)
            Button("Open benchmark") { model.page = .benchmark }.buttonStyle(
              LabButtonStyle(prominent: true))
          }.frame(maxWidth: .infinity).padding(.vertical, 34)
        }
      }
      if !model.history.isEmpty { history }
    }
  }
  private var history: some View {
    LabCard {
      VStack(alignment: .leading, spacing: 16) {
        HStack {
          Eyebrow(text: "Local history")
          Spacer()
          Text("\(model.history.count) runs").font(.system(size: 11, design: .monospaced))
            .foregroundStyle(Color.labMuted)
        }
        ForEach(model.history) { entry in
          Button {
            model.select(entry)
          } label: {
            HStack(spacing: 14) {
              Circle().fill(entry.result?.accepted == true ? Color.labGreen : .labCoral).frame(
                width: 6, height: 6)
              VStack(alignment: .leading, spacing: 4) {
                Text(entry.result?.report.kind.capitalized ?? "Unreadable result").font(
                  .custom("AvenirNext-DemiBold", size: 13))
                Text(entry.date.formatted(date: .abbreviated, time: .shortened)).font(
                  .system(size: 11)
                ).foregroundStyle(Color.labMuted)
              }
              Spacer()
              Text(
                entry.result?.score.map { String(format: "%.1f FPS", $0) }
                  ?? (entry.result?.accepted == true ? "Completed" : "Not qualified")
              ).font(.system(size: 12, design: .monospaced)).foregroundStyle(Color.labMuted)
              Image(systemName: "arrow.up.right").font(.system(size: 10)).foregroundStyle(
                Color.labMuted)
            }.padding(12).background(
              model.selectedHistoryID == entry.id ? Color.labRaised : .clear,
              in: RoundedRectangle(cornerRadius: 10)
            ).contentShape(Rectangle())
          }.buttonStyle(.plain)
        }
      }
    }
  }
}
struct CaptureComparison: View {
  let result: LoadedReport
  @State private var leftID = ""
  @State private var rightID = ""
  @State private var sampleID = ""
  private struct CaptureArm: Identifiable {
    let id: String
    let title: String
    let captures: [CaptureReference]
  }
  private var arms: [CaptureArm] {
    if !result.arms.isEmpty {
      var seen = Set<String>()
      return result.arms.compactMap { arm in
        guard seen.insert(arm.arm.label).inserted, let captures = arm.arm.captures,
          !captures.isEmpty
        else { return nil }
        return CaptureArm(id: arm.arm.label, title: arm.arm.label, captures: captures)
      }
    }
    return []
  }
  private var left: CaptureArm? { arms.first(where: { $0.id == leftID }) ?? arms.first }
  private var right: CaptureArm? {
    arms.first(where: { $0.id == rightID }) ?? arms.dropFirst().first
  }
  private var samples: [CaptureReference] {
    guard let left, let right else { return [] }
    let keys = Set(right.captures.map(\.pairingKey))
    return left.captures.filter { keys.contains($0.pairingKey) }.sorted {
      ($0.scene, $0.tick) < ($1.scene, $1.tick)
    }
  }
  var body: some View {
    if arms.count >= 2 {
      LabCard {
        VStack(alignment: .leading, spacing: 17) {
          HStack {
            Eyebrow(text: "Inspect the retained pixels")
            Spacer()
            Text("Drag the divider").font(.system(size: 11)).foregroundStyle(Color.labMuted)
          }
          HStack {
            Picker("Left", selection: $leftID) { ForEach(arms) { Text($0.title).tag($0.id) } }
              .labelsHidden()
            Picker("Right", selection: $rightID) { ForEach(arms) { Text($0.title).tag($0.id) } }
              .labelsHidden()
            Spacer()
            Picker("Frame", selection: $sampleID) {
              ForEach(samples, id: \.pairingKey) {
                Text("\($0.scene.capitalized) · \($0.tick)").tag($0.pairingKey)
              }
            }.labelsHidden()
          }
          if let a = samples.first(where: { $0.pairingKey == sampleID }) ?? samples.first,
            let b = right?.captures.first(where: { $0.pairingKey == a.pairingKey }),
            let leftURL = try? result.imageURL(a), let rightURL = try? result.imageURL(b)
          {
            ImageDivider(
              leftURL: leftURL, rightURL: rightURL, leftLabel: left?.title ?? "",
              rightLabel: right?.title ?? "")
          } else {
            Text("No matching retained scene and frame pair is available.").foregroundStyle(
              Color.labMuted
            ).frame(height: 100)
          }
          Text("Identical scene and timeline tick. Captures are separate from the scored run.")
            .font(.system(size: 10)).foregroundStyle(Color.labMuted)
        }
      }.onAppear {
        leftID = arms.first?.id ?? ""
        rightID = arms.dropFirst().first?.id ?? ""
        sampleID = samples.first?.pairingKey ?? ""
      }
    } else if !result.arms.isEmpty {
      Text("This report has no matching image pair to inspect.").font(.system(size: 12))
        .foregroundStyle(Color.labMuted)
    }
  }
}
struct ImageDivider: View {
  let leftURL: URL
  let rightURL: URL
  let leftLabel: String
  let rightLabel: String
  @State private var fraction = 0.5
  @State private var leftImage: NSImage?
  @State private var rightImage: NSImage?
  var body: some View {
    VStack(spacing: 10) {
      GeometryReader { geometry in
        ZStack(alignment: .leading) {
          Color.black
          if let rightImage {
            Image(nsImage: rightImage).resizable().scaledToFit().frame(
              width: geometry.size.width, height: geometry.size.height)
          }
          if let leftImage {
            Image(nsImage: leftImage).resizable().scaledToFit().frame(
              width: geometry.size.width, height: geometry.size.height
            ).mask(alignment: .leading) { Rectangle().frame(width: geometry.size.width * fraction) }
          }
          Rectangle().fill(Color.labInk).frame(width: 2).offset(
            x: geometry.size.width * fraction - 1)
          Image(systemName: "arrow.left.and.right").font(.system(size: 12, weight: .bold))
            .foregroundStyle(Color.labBackground).padding(11).background(Color.labInk, in: Circle())
            .offset(x: geometry.size.width * fraction - 19)
          VStack {
            HStack {
              Text(leftLabel)
              Spacer()
              Text(rightLabel)
            }.font(.system(size: 10, weight: .semibold)).padding(11).background(
              .black.opacity(0.48))
            Spacer()
          }
        }.clipShape(RoundedRectangle(cornerRadius: 10)).contentShape(Rectangle())
          .gesture(
            DragGesture(minimumDistance: 0).onChanged {
              fraction = min(1, max(0, $0.location.x / geometry.size.width))
            }
          )
          .accessibilityLabel(
            "Image comparison: \(leftLabel) on the left, \(rightLabel) on the right")
      }.aspectRatio(16 / 9, contentMode: .fit)
      Slider(value: $fraction, in: 0...1).accessibilityLabel("Comparison divider")
    }.task(id: leftURL.path + rightURL.path) {
      leftImage = NSImage(contentsOf: leftURL)
      rightImage = NSImage(contentsOf: rightURL)
      fraction = 0.5
    }
  }
}
