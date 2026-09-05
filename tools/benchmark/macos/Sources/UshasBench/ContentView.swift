import BenchCore
import SwiftUI

struct ContentView: View {
  @Bindable var model: BenchModel
  var body: some View {
    HStack(spacing: 0) {
      sidebar.frame(width: 184)
      Rectangle().fill(Color.white.opacity(0.06)).frame(width: 1)
      VStack(alignment: .leading, spacing: 0) {
        header
        ScrollView {
          VStack(alignment: .leading, spacing: 20) {
            if let error = model.error {
              HStack(alignment: .top) {
                Image(systemName: "exclamationmark.circle")
                Text(error).textSelection(.enabled)
                Spacer()
                Button {
                  model.error = nil
                } label: {
                  Image(systemName: "xmark")
                }.buttonStyle(.plain)
              }
              .font(.system(size: 12)).foregroundStyle(Color.labCoral).padding(16).background(
                Color.labCoral.opacity(0.08), in: RoundedRectangle(cornerRadius: 10))
            }
            if model.running { runningCard }
            switch model.page {
            case .benchmark: benchmarkPage
            case .compare: comparePage
            case .stress: stressPage
            case .results: ResultsView(model: model)
            }
          }.padding(28)
        }
      }
    }
    .background(Color.labBackground).foregroundStyle(Color.labInk)
    .frame(minWidth: 1020, minHeight: 720)
    .tint(.labCoral)
  }
  private var sidebar: some View {
    VStack(alignment: .leading, spacing: 0) {
      HStack(spacing: 10) {
        ClaudeMark().frame(width: 36, height: 36)
        Text("USHAS").font(.custom("AvenirNext-Heavy", size: 20)).tracking(2)
      }.padding(.top, 45).padding(.bottom, 38)
      Eyebrow(text: "Render lab").padding(.bottom, 17)
      ForEach(Array(BenchModel.Page.allCases.enumerated()), id: \.element.id) { index, page in
        Button {
          model.page = page
        } label: {
          HStack(spacing: 11) {
            Text(String(format: "%02d", index + 1)).font(.system(size: 10, design: .monospaced))
              .foregroundStyle(model.page == page ? Color.labCoral : .labMuted)
            Text(page.rawValue).font(.custom("AvenirNext-DemiBold", size: 13))
            Spacer()
            if model.page == page { Circle().fill(Color.labCoral).frame(width: 4, height: 4) }
          }.padding(.horizontal, 10).padding(.vertical, 13).contentShape(Rectangle())
            .background(
              model.page == page ? Color.labRaised : .clear, in: RoundedRectangle(cornerRadius: 8))
        }.buttonStyle(.plain).padding(.bottom, 6)
      }
      Spacer(minLength: 80)
      VStack(alignment: .leading, spacing: 7) {
        Text("LOCAL BY DESIGN").font(.system(size: 9, weight: .semibold, design: .monospaced))
          .tracking(1.4)
        Text("Your runs stay here.\nReports travel offline.").font(
          .custom("AvenirNext-Regular", size: 11)
        ).lineSpacing(3)
        Text("Claude by vgel").font(.system(size: 9)).padding(.top, 12)
      }.foregroundStyle(Color.labMuted).padding(.bottom, 24)
    }.padding(.horizontal, 18)
  }
  private var header: some View {
    HStack(alignment: .center) {
      VStack(alignment: .leading, spacing: 5) {
        Eyebrow(text: "Apple Silicon / Preview 01")
        Text(model.page.rawValue).font(.custom("AvenirNext-DemiBold", size: 28))
      }
      Spacer()
      StatusPill(title: model.running ? "LAB RUNNING" : "READY", good: !model.running)
    }.padding(.horizontal, 28).padding(.top, 31).padding(.bottom, 18)
  }
  private var runningCard: some View {
    LabCard(padding: 18) {
      HStack(spacing: 18) {
        ProgressView().controlSize(.small)
        VStack(alignment: .leading, spacing: 7) {
          Text(model.currentScene).font(.custom("AvenirNext-DemiBold", size: 14))
          Text(model.status).font(.system(size: 11)).foregroundStyle(Color.labMuted)
          ProgressView(value: model.progress).tint(.labCoral).frame(maxWidth: 360)
        }
        Spacer()
        if model.activeCommand == "stress" {
          Button("Live controls") { model.showStressControls() }.buttonStyle(LabButtonStyle())
        }
        Button("Stop") { model.stop() }.buttonStyle(LabButtonStyle())
      }
    }
  }
  private var hero: some View {
    HStack(spacing: 12) {
      VStack(alignment: .leading, spacing: 16) {
        Eyebrow(text: "Character under pressure")
        Text("A small character.\nA demanding render.").font(
          .custom("AvenirNext-DemiBold", size: 33)
        ).lineSpacing(-2).fixedSize(horizontal: false, vertical: true)
        Text("Three deterministic scenes. One clear view of how your Mac renders them.").font(
          .custom("AvenirNext-Regular", size: 13)
        ).foregroundStyle(Color.labMuted).lineSpacing(4).frame(maxWidth: 360, alignment: .leading)
        HStack(spacing: 8) {
          StatusPill(title: "1440p")
          StatusPill(title: "3 SCENES")
          StatusPill(title: "1200 FRAMES EACH")
        }
      }.padding(28)
      Spacer(minLength: 0)
      ZStack {
        LabGrid()
        ClaudeMark().frame(width: 210, height: 210).rotationEffect(.degrees(-8))
      }.frame(width: 245, height: 260).clipped()
    }.background(Color.labSurface, in: RoundedRectangle(cornerRadius: 16)).clipShape(
      RoundedRectangle(cornerRadius: 16))
  }
  private var benchmarkPage: some View {
    VStack(spacing: 20) {
      hero
      HStack(alignment: .top, spacing: 18) {
        VStack(spacing: 12) {
          sceneRow(
            number: "01", title: "Materials", detail: "Hero Claude · 12 material studies",
            symbol: "circle.lefthalf.filled")
          sceneRow(
            number: "02", title: "Geometry", detail: "64 characters · fine structures",
            symbol: "cube.transparent")
          sceneRow(
            number: "03", title: "Lighting", detail: "Shadows · particles · moving light",
            symbol: "light.max")
        }.frame(maxWidth: .infinity)
        LabCard {
          VStack(alignment: .leading, spacing: 17) {
            Eyebrow(text: "Render configuration")
            modeControls
            Text("Standard profile · 120 Hz simulation timeline. Requests uncapped presentation.").font(
              .system(size: 11)
            ).foregroundStyle(Color.labMuted).fixedSize(horizontal: false, vertical: true)
            Button {
              model.launch("benchmark")
            } label: {
              HStack {
                Text("Run benchmark")
                Spacer()
                Image(systemName: "arrow.up.right")
              }
            }.buttonStyle(LabButtonStyle(prominent: true)).disabled(model.running)
            Text("The lab opens in its own window. Results return here.").font(.system(size: 10))
              .foregroundStyle(Color.labMuted)
          }
        }.frame(width: 294)
      }
    }
  }
  private func sceneRow(number: String, title: String, detail: String, symbol: String) -> some View
  {
    LabCard(padding: 17) {
      HStack(spacing: 14) {
        Text(number).font(.system(size: 12, design: .monospaced)).foregroundStyle(Color.labCoral)
        Image(systemName: symbol).font(.system(size: 23, weight: .ultraLight)).frame(width: 35)
          .foregroundStyle(Color.labMuted)
        VStack(alignment: .leading, spacing: 4) {
          Text(title).font(.custom("AvenirNext-DemiBold", size: 14))
          Text(detail).font(.system(size: 11)).foregroundStyle(Color.labMuted)
        }
        Spacer()
      }
    }
  }
  private var modeControls: some View {
    VStack(alignment: .leading, spacing: 14) {
      Picker("Mode", selection: $model.configuration.mode) {
        ForEach(RenderMode.allCases) { Text($0.title).tag($0) }
      }.pickerStyle(.menu)
        .onChange(of: model.configuration.mode) { _, mode in
          if mode == .native { model.configuration.scale = "1" }
        }
      Picker("Input scale", selection: $model.configuration.scale) {
        Text("Full").tag("1")
        Text("Two-thirds").tag("2/3")
        Text("Half").tag("1/2")
      }.pickerStyle(.menu).disabled(model.configuration.mode == .native)
      Text(
        model.configuration.mode == .native
          ? "Native uses full resolution and MSAA 4×."
          : "Output stays at 2560 × 1440. Input scale changes reconstruction cost."
      ).font(.system(size: 11)).foregroundStyle(Color.labMuted).fixedSize(
        horizontal: false, vertical: true)
    }.disabled(model.running)
  }
  private var comparePage: some View {
    VStack(alignment: .leading, spacing: 20) {
      HStack(alignment: .bottom) {
        VStack(alignment: .leading, spacing: 10) {
          Eyebrow(text: "Same scene. Six perspectives.")
          Text("Find what reconstruction buys.").font(.custom("AvenirNext-DemiBold", size: 30))
          Text(
            "A native reference, five alternatives, and retained images you can inspect side by side."
          ).font(.custom("AvenirNext-Regular", size: 13)).foregroundStyle(Color.labMuted)
        }
        Spacer()
        ClaudeMark().frame(width: 88, height: 88)
      }.padding(.vertical, 12)
      LazyVGrid(
        columns: [GridItem(.flexible()), GridItem(.flexible()), GridItem(.flexible())], spacing: 13
      ) {
        ForEach(
          Array(
            [
              ("Native", "Full · MSAA 4×"), ("Temporal", "Full"), ("Temporal", "Two-thirds"),
              ("Temporal", "Half"), ("Spatial", "Half"), ("Bilinear", "Half"),
            ].enumerated()), id: \.offset
        ) { index, entry in
          LabCard {
            VStack(alignment: .leading, spacing: 13) {
              Text(String(format: "%02d", index + 1)).font(.system(size: 11, design: .monospaced))
                .foregroundStyle(Color.labCoral)
              Text(entry.0).font(.custom("AvenirNext-DemiBold", size: 19))
              Text(entry.1).font(.system(size: 12)).foregroundStyle(Color.labMuted)
            }.frame(maxWidth: .infinity, alignment: .leading)
          }
        }
      }
      LabCard {
        HStack(alignment: .center, spacing: 25) {
          VStack(alignment: .leading, spacing: 10) {
            Picker("Comparison", selection: $model.configuration.rounds) {
              Text("Quick · 6 measurements").tag(1)
              Text("Qualification · 24 measurements").tag(4)
            }.pickerStyle(.menu).frame(width: 310)
            Text(
              model.configuration.rounds == 1
                ? "A first look at each mode. Use qualification before drawing a performance conclusion."
                : "Four balanced rounds with paired uncertainty and an 8% practical-benefit threshold."
            ).font(.system(size: 12)).foregroundStyle(Color.labMuted).frame(
              maxWidth: 420, alignment: .leading)
          }
          Spacer()
          Button("Run comparison") { model.launch("compare") }.buttonStyle(
            LabButtonStyle(prominent: true))
        }.disabled(model.running)
      }
      Text(
        "Image replays run separately from scored measurements. Comparing images never starts another render."
      ).font(.system(size: 11)).foregroundStyle(Color.labMuted)
    }
  }
  private var stressPage: some View {
    HStack(alignment: .top, spacing: 20) {
      VStack(alignment: .leading, spacing: 20) {
        Text("Turn up the pressure.").font(.custom("AvenirNext-DemiBold", size: 32))
        Text(
          "Explore a custom workload for up to ten minutes. Change the load live, watch completed-render rate, and stop whenever you need."
        ).font(.custom("AvenirNext-Regular", size: 14)).foregroundStyle(Color.labMuted).lineSpacing(
          5)
        ZStack {
          LabGrid()
          HStack(spacing: -18) {
            ClaudeMark().frame(width: 100, height: 100).rotationEffect(.degrees(-13))
            ClaudeMark().frame(width: 170, height: 170)
            ClaudeMark().frame(width: 100, height: 100).rotationEffect(.degrees(13))
          }
        }.frame(height: 255).background(Color.labSurface, in: RoundedRectangle(cornerRadius: 16))
          .clipped()
        Text(
          "Custom stress runs have no benchmark score. The floating controls stay available over the lab."
        ).font(.system(size: 11)).foregroundStyle(Color.labMuted)
      }.frame(maxWidth: .infinity, alignment: .leading)
      LabCard {
        VStack(alignment: .leading, spacing: 17) {
          modeControls
          StressSliders(model: model)
          Button(model.running ? "Show live controls" : "Start 10-minute stress") {
            if model.running { model.showStressControls() } else { model.launch("stress") }
          }.buttonStyle(LabButtonStyle(prominent: true)).disabled(
            model.running && model.activeCommand != "stress")
        }
      }.frame(width: 310)
    }
  }
}
struct StressSliders: View {
  @Bindable var model: BenchModel
  var body: some View {
    VStack(spacing: 18) {
      control("Claudes", value: $model.configuration.claudes, range: 1...256, step: 1)
      control("Lights", value: $model.configuration.lights, range: 1...32, step: 1)
      control("Particles", value: $model.configuration.particles, range: 0...32768, step: 512)
      control("Extra pixel load", value: $model.configuration.fill, range: 0...16, step: 1)
    }
  }
  private func control(_ title: String, value: Binding<Int>, range: ClosedRange<Int>, step: Int)
    -> some View
  {
    VStack(spacing: 5) {
      HStack {
        Text(title).font(.custom("AvenirNext-Medium", size: 12))
        Spacer()
        Text(value.wrappedValue.formatted()).font(.system(size: 12, design: .monospaced))
          .foregroundStyle(Color.labCoral)
      }
      Slider(
        value: Binding(
          get: { Double(value.wrappedValue) },
          set: {
            value.wrappedValue = Int($0)
            model.configureStress()
          }), in: Double(range.lowerBound)...Double(range.upperBound), step: Double(step)
      ).tint(.labCoral)
    }
  }
}
struct StressControlPanel: View {
  @Bindable var model: BenchModel
  var body: some View {
    VStack(alignment: .leading, spacing: 21) {
      HStack {
        Eyebrow(text: "Live stress / Custom")
        Spacer()
        ClaudeMark().frame(width: 35, height: 35)
      }
      HStack(alignment: .firstTextBaseline) {
        Text(model.liveFPS.map { String(format: "%.1f", $0) } ?? "—").font(
          .system(size: 42, weight: .light, design: .monospaced))
        Text("render FPS").font(.system(size: 11)).foregroundStyle(Color.labMuted)
      }
      StressSliders(model: model)
      Text(model.status).font(.system(size: 11)).foregroundStyle(Color.labMuted).lineLimit(2)
      Button("Stop and save") { model.stop() }.buttonStyle(LabButtonStyle(prominent: true)).frame(
        maxWidth: .infinity)
    }.padding(23).background(Color.labBackground).foregroundStyle(Color.labInk)
      .preferredColorScheme(.dark)
  }
}
