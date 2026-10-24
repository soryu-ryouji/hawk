import UIKit

/// 触感反馈：只用在「确实发生了状态变化」的地方（保存成功、解锁成功、收藏切换），
/// 滚动与普通点击不触发，避免手机持续震动。
@MainActor
enum Haptics {
    static func tap() {
        UIImpactFeedbackGenerator(style: .light).impactOccurred()
    }

    static func success() {
        UINotificationFeedbackGenerator().notificationOccurred(.success)
    }

    static func warning() {
        UINotificationFeedbackGenerator().notificationOccurred(.warning)
    }
}
