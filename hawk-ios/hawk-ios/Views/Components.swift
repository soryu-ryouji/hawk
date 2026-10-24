import SwiftUI

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

struct ErrorBanner: View {
    let message: String
    var retry: (() -> Void)?

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundStyle(HawkTheme.danger)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 6) {
                Text(message)
                    .font(.subheadline)
                    .foregroundStyle(HawkTheme.fg0)
                    .fixedSize(horizontal: false, vertical: true)
                if let retry {
                    Button("重试", action: retry)
                        .font(.subheadline.weight(.medium))
                }
            }
            Spacer(minLength: 0)
        }
        .padding(12)
        .background(HawkTheme.danger.opacity(0.12), in: RoundedRectangle(cornerRadius: 10))
        .overlay(
            RoundedRectangle(cornerRadius: 10).stroke(HawkTheme.danger.opacity(0.35), lineWidth: 1)
        )
    }
}

/// 轻量提示条：保存成功、已重新锁定一类的一次性反馈（可点击消失）
struct HawkToast: View {
    let text: String
    var symbol: String = "checkmark.circle.fill"

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: symbol)
                .foregroundStyle(HawkTheme.accent)
            Text(text)
                .font(.subheadline)
                .foregroundStyle(HawkTheme.fg0)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .background(.ultraThinMaterial, in: Capsule())
        .overlay(Capsule().stroke(HawkTheme.border, lineWidth: 1))
        .shadow(radius: 8, y: 4)
        .transition(.move(edge: .bottom).combined(with: .opacity))
    }
}

struct FilterChip: View {
    let label: String
    var tint: Color = HawkTheme.accent
    var showsDot = false
    let clear: () -> Void

    var body: some View {
        HStack(spacing: 6) {
            if showsDot {
                Circle()
                    .fill(tint)
                    .frame(width: 12, height: 12)
                    .overlay(Circle().stroke(.white.opacity(0.25), lineWidth: 1))
            }
            Text(label)
                .font(.footnote)
            Button(action: clear) {
                Image(systemName: "xmark")
                    .font(.system(size: 9, weight: .bold))
                    .padding(2)
            }
            .buttonStyle(.plain)
            .accessibilityLabel("清除\(label)筛选")
        }
        .foregroundStyle(HawkTheme.fg0)
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .background(HawkTheme.bg3, in: Capsule())
    }
}

/// 侧栏/列表行尾的计数
struct CountBadge: View {
    let value: Int?

    var body: some View {
        Text(value.map(String.init) ?? "—")
            .font(.caption)
            .monospacedDigit()
            .foregroundStyle(HawkTheme.fg1)
            .contentTransition(.numericText())
            .accessibilityLabel(value.map { "\($0) 项" } ?? "计数加载中")
    }
}

struct LockBadge: View {
    var unlocked = false

    var body: some View {
        Image(systemName: unlocked ? "lock.open.fill" : "lock.fill")
            .font(.caption2)
            .foregroundStyle(unlocked ? HawkTheme.accent : HawkTheme.fg1)
            .accessibilityLabel(unlocked ? "已解锁" : "已锁定")
    }
}

/// 通用分区标题（列表与表单之外的场景）
struct SectionCaption: View {
    let text: String

    var body: some View {
        Text(text)
            .font(.footnote.weight(.semibold))
            .foregroundStyle(HawkTheme.fg1)
            .textCase(nil)
    }
}

extension View {
    /// 底部一次性提示；`text` 变化时自动消失
    func hawkToast(_ text: Binding<String?>, symbol: String = "checkmark.circle.fill") -> some View
    {
        overlay(alignment: .bottom) {
            if let value = text.wrappedValue {
                HawkToast(text: value, symbol: symbol)
                    .padding(.bottom, 24)
                    .onAppear {
                        Task { @MainActor in
                            try? await Task.sleep(nanoseconds: 1_800_000_000)
                            withAnimation { text.wrappedValue = nil }
                        }
                    }
                    .allowsHitTesting(false)
            }
        }
        .animation(.easeOut(duration: 0.2), value: text.wrappedValue)
    }
}
