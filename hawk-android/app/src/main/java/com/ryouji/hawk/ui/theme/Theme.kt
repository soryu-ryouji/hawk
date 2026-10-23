package com.ryouji.hawk.ui.theme

import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Shapes
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.unit.dp

private val HawkColorScheme = darkColorScheme(
    primary = HawkColors.Accent,
    onPrimary = androidx.compose.ui.graphics.Color.White,
    primaryContainer = HawkColors.Bg3,
    onPrimaryContainer = HawkColors.Fg0,
    secondary = HawkColors.Fg1,
    onSecondary = HawkColors.Bg0,
    background = HawkColors.Bg0,
    onBackground = HawkColors.Fg0,
    surface = HawkColors.Bg1,
    onSurface = HawkColors.Fg0,
    surfaceVariant = HawkColors.Bg2,
    onSurfaceVariant = HawkColors.Fg1,
    surfaceContainer = HawkColors.Bg1,
    surfaceContainerHigh = HawkColors.Bg2,
    surfaceContainerHighest = HawkColors.Bg3,
    outline = HawkColors.Border,
    outlineVariant = HawkColors.Border,
    error = HawkColors.Danger,
    onError = androidx.compose.ui.graphics.Color.White,
)

private val HawkShapes = Shapes(
    extraSmall = RoundedCornerShape(4.dp),
    small = RoundedCornerShape(6.dp),
    medium = RoundedCornerShape(10.dp),
    large = RoundedCornerShape(14.dp),
)

@Composable
fun HawkTheme(content: @Composable () -> Unit) {
    MaterialTheme(
        colorScheme = HawkColorScheme,
        shapes = HawkShapes,
        typography = HawkTypography,
        content = content,
    )
}
