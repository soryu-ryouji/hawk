import SwiftUI

struct RootView: View {
    @Environment(AppModel.self) private var app
    @Environment(\.scenePhase) private var scenePhase
    @State private var preview = PreviewCoordinator()

    var body: some View {
        ZStack {
            content
            // 预览在根层：网格、导航栏、Tab 栏都留在下面，下拉时遮罩淡出即可露出（相册行为）
            if let session = preview.session {
                PreviewView(session: session)
                    .environment(preview)
                    .zIndex(1)
            }
        }
        .task { app.start() }
        .onChange(of: scenePhase) { _, phase in
            app.setBackgrounded(phase != .active)
            if phase == .active {
                Task { await app.context?.refreshIfStale() }
            }
        }
        .environment(preview)
    }

    @ViewBuilder
    private var content: some View {
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
