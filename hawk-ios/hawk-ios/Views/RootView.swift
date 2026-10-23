import SwiftUI

struct RootView: View {
    @Environment(AppModel.self) private var app

    var body: some View {
        Group {
            switch app.phase {
            case .launching:
                LaunchScreen()
            case .connect:
                ConnectView()
            case .connecting(let message):
                ConnectingScreen(message: message)
            case .ready:
                if let session = app.session {
                    AdaptiveLibraryView(session: session)
                } else {
                    ConnectView()
                }
            }
        }
        .task { app.start() }
    }
}

private struct LaunchScreen: View {
    var body: some View {
        VStack(spacing: 18) {
            HawkMark(size: 88)
            Text("hawk")
                .font(.system(size: 22, weight: .semibold))
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
    let message: String

    var body: some View {
        VStack(spacing: 20) {
            HawkMark(size: 76)
            VStack(spacing: 8) {
                Text(message)
                    .font(.system(size: 15, weight: .medium))
                    .foregroundStyle(HawkTheme.fg0)
                Text("正在读取素材库信息；进入后会先显示首屏，其余内容在后台补齐")
                    .font(.system(size: 12))
                    .foregroundStyle(HawkTheme.fg1)
            }
            ProgressView()
                .tint(HawkTheme.accent)
            Button("取消") {
                app.disconnect()
            }
            .font(.system(size: 13))
            .foregroundStyle(HawkTheme.fg1)
            .padding(.top, 8)
        }
        .padding(28)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .hawkScreenBackground()
    }
}

struct HawkMark: View {
    var size: CGFloat

    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: size * 0.24, style: .continuous)
                .fill(
                    LinearGradient(
                        colors: [Color(hex: "#4f8cff"), Color(hex: "#2f5fd0")],
                        startPoint: .topLeading,
                        endPoint: .bottomTrailing
                    )
                )
            Image(systemName: "bird.fill")
                .font(.system(size: size * 0.46, weight: .semibold))
                .foregroundStyle(.white)
        }
        .frame(width: size, height: size)
        .shadow(color: .black.opacity(0.35), radius: size * 0.12, y: size * 0.05)
        .accessibilityHidden(true)
    }
}
