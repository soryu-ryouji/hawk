package com.ryouji.hawk.ui.theme

import androidx.compose.ui.graphics.Color

/** 与 hawk-app/web/src/app/styles.css 同一套深色变量。 */
object HawkColors {
    val Bg0 = Color(0xFF1E1E1E)
    val Bg1 = Color(0xFF252526)
    val Bg2 = Color(0xFF2D2D30)
    val Bg3 = Color(0xFF3A3A3D)
    val Fg0 = Color(0xFFE8E8E8)
    val Fg1 = Color(0xFF9D9D9D)
    val Accent = Color(0xFF4F8CFF)
    val Danger = Color(0xFFE5534B)
    val Border = Color(0xFF3C3C3C)
    val Star = Color(0xFFF5C518)

    /** 颜色筛选的常用色（与桌面端调色板检索同一语义）。 */
    val FilterColors = listOf(
        "#e5534b",
        "#e8a33d",
        "#e5d54b",
        "#4caf50",
        "#4a9eff",
        "#8a6bff",
        "#e06bb5",
        "#e8e8e8",
        "#1e1e1e",
        "#8b5a2b",
    )
}

/** 把 #rrggbb / #aarrggbb 解析成 Color。 */
fun Color.Companion.fromHex(hex: String): Color {
    val cleaned = hex.trim().removePrefix("#")
    val value = cleaned.toLongOrNull(16) ?: return Color(0xFF808080)
    return when (cleaned.length) {
        3 -> {
            val r = ((value shr 8) and 0xF) * 17
            val g = ((value shr 4) and 0xF) * 17
            val b = (value and 0xF) * 17
            Color(0xFF000000 or (r shl 16) or (g shl 8) or b)
        }
        6 -> Color(0xFF000000 or value)
        8 -> Color(value)
        else -> Color(0xFF808080)
    }
}
