import SwiftUI

/// 与 hawk-app/web/src/app/styles.css 同一套深色变量，移动端沿用桌面/web 观感。
enum HawkTheme {
    static let bg0 = Color(hex: "#1e1e1e")
    static let bg1 = Color(hex: "#252526")
    static let bg2 = Color(hex: "#2d2d30")
    static let bg3 = Color(hex: "#3a3a3d")
    static let fg0 = Color(hex: "#e8e8e8")
    static let fg1 = Color(hex: "#9d9d9d")
    static let accent = Color(hex: "#4f8cff")
    static let danger = Color(hex: "#e5534b")
    static let border = Color(hex: "#3c3c3c")
    static let star = Color(hex: "#f5c518")

    static let cornerRadius: CGFloat = 10
    static let cardRadius: CGFloat = 8
    static let screenPadding: CGFloat = 16
    static let gridGap: CGFloat = 10

    /// 颜色筛选的常用色（与网页端调色板检索同一语义：CIE76 近似命中）。
    static let filterColors: [String] = [
        "#e5534b", "#e8a33d", "#e5d54b", "#4caf50", "#4a9eff",
        "#8a6bff", "#e06bb5", "#e8e8e8", "#1e1e1e", "#8b5a2b",
    ]
}

extension Color {
    init(hex: String) {
        let cleaned = hex.trimmingCharacters(in: CharacterSet.alphanumerics.inverted)
        var value: UInt64 = 0
        Scanner(string: cleaned).scanHexInt64(&value)
        let r: UInt64
        let g: UInt64
        let b: UInt64
        let a: UInt64
        switch cleaned.count {
        case 3:
            (r, g, b, a) = ((value >> 8) * 17, (value >> 4 & 0xF) * 17, (value & 0xF) * 17, 255)
        case 6:
            (r, g, b, a) = (value >> 16, value >> 8 & 0xFF, value & 0xFF, 255)
        case 8:
            (r, g, b, a) = (value >> 24, value >> 16 & 0xFF, value >> 8 & 0xFF, value & 0xFF)
        default:
            (r, g, b, a) = (128, 128, 128, 255)
        }
        self.init(
            .sRGB,
            red: Double(r) / 255,
            green: Double(g) / 255,
            blue: Double(b) / 255,
            opacity: Double(a) / 255
        )
    }
}

extension View {
    func hawkScreenBackground() -> some View {
        background(HawkTheme.bg0.ignoresSafeArea())
    }

    /// 分组容器：与桌面端卡片一致的深色块
    func hawkCard(padding: CGFloat = 14) -> some View {
        self.padding(padding)
            .background(HawkTheme.bg1, in: RoundedRectangle(cornerRadius: HawkTheme.cornerRadius))
            .overlay(
                RoundedRectangle(cornerRadius: HawkTheme.cornerRadius)
                    .stroke(HawkTheme.border, lineWidth: 1)
            )
    }
}
