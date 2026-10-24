import SwiftUI

struct RootView: View {
    @Environment(AppModel.self) private var app
    @Environment(\.scenePhase) private var scenePhase

    var body: some View {
        Group {
            switch app.phase {
            case .launching:
                LaunchScreen()
            case .connect:
                ConnectView()
            case .connecting:
                ConnectingScreen()
            case .ready:
                if let context = app.context {
                    AdaptiveLibraryView(context: context)
                } else {
                    ConnectView()
                }
            }
        }
        .task { app.start() }
        .onChange(of: scenePhase) { _, phase in
            app.setBackgrounded(phase != .active)
            if phase == .active {
                Task { await app.context?.refreshIfStale() }
            }
        }
    }
}

private struct LaunchScreen: View {
    var body: some View {
        VStack(spacing: 18) {
            HawkMark(size: 88)
            Text("hawk")
                .font(.title2.weight(.semibold))
                .foregroundStyle(HawkTheme.fg0)
            ProgressView()
                .tint(HawkTheme.fg1)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .hawkScreenBackground()
    }
}

private struct ConnectingScreen: View {
    @Environment(AppModel.self) private var app

    var body: some View {
        VStack(spacing: 22) {
            HawkMark(size: 76)
            VStack(spacing: 8) {
                Text(app.connectStatus ?? "正在连接…")
                    .font(.headline)
                    .foregroundStyle(HawkTheme.fg0)
                    .multilineTextAlignment(.center)
                Text("进入后会先显示首屏，其余内容在后台补齐")
                    .font(.footnote)
                    .foregroundStyle(HawkTheme.fg1)
            }
            ProgressView()
                .tint(HawkTheme.accent)
            Button("取消") {
                app.disconnect()
            }
            .font(.subheadline)
            .foregroundStyle(HawkTheme.fg1)
            .padding(.top, 4)
        }
        .padding(28)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .hawkScreenBackground()
    }
}
