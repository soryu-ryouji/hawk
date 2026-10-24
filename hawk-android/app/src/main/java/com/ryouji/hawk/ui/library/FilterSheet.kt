package com.ryouji.hawk.ui.library

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.ryouji.hawk.data.library.LibraryStore
import com.ryouji.hawk.data.model.SizeFilter
import com.ryouji.hawk.ui.theme.HawkColors
import com.ryouji.hawk.ui.theme.fromHex

private val TIERS = listOf(480, 720, 1080, 1440, 2160, 4320)

@OptIn(ExperimentalMaterial3Api::class, ExperimentalLayoutApi::class)
@Composable
fun FilterSheet(store: LibraryStore, onDismiss: () -> Unit) {
    val state by store.state.collectAsStateWithLifecycle()
    val query = state.query
    var star by remember { mutableStateOf(query.star) }
    var color by remember { mutableStateOf(query.color) }
    var minSide by remember {
        mutableStateOf((query.size as? SizeFilter.Side)?.min?.toString().orEmpty())
    }
    var maxSide by remember {
        mutableStateOf((query.size as? SizeFilter.Side)?.max?.toString().orEmpty())
    }
    var sizeError by remember { mutableStateOf<String?>(null) }

    fun reset() {
        star = null
        color = null
        minSide = ""
        maxSide = ""
        sizeError = null
        store.clearFilters()
        onDismiss()
    }

    fun apply() {
        sizeError = null
        val minValue = parseSide(minSide)
        val maxValue = parseSide(maxSide)
        if (sizeError != null) return
        if (minValue != null && maxValue != null && minValue > maxValue) {
            sizeError = "最小短边不能大于最大短边"
            return
        }
        val size = if (minValue == null && maxValue == null) {
            null
        } else {
            SizeFilter.Side(minValue, maxValue)
        }
        store.applyFilters(star = star, color = color, size = size)
        onDismiss()
    }

    ModalBottomSheet(
        onDismissRequest = onDismiss,
        containerColor = HawkColors.Bg1,
    ) {
        Box(
            modifier = Modifier.fillMaxWidth(),
            contentAlignment = Alignment.TopCenter,
        ) {
            Column(
                modifier = Modifier
                    .widthIn(max = 560.dp)
                    .fillMaxWidth()
                    .verticalScroll(rememberScrollState())
                    .padding(horizontal = 20.dp)
                    .padding(bottom = 28.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    text = "筛选",
                    color = HawkColors.Fg0,
                    fontSize = 16.sp,
                    fontWeight = FontWeight.SemiBold,
                    modifier = Modifier.weight(1f),
                )
                TextButton(onClick = { reset() }) { Text("重置", color = HawkColors.Fg1) }
            }

            Text("评分", color = HawkColors.Fg1, fontSize = 12.sp)
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                FilterChip(label = "全部", selected = star == null, onClick = { star = null })
                FilterChip(label = "未评分", selected = star == 0, onClick = { star = 0 })
                (1..5).forEach { value ->
                    FilterChip(
                        label = "★".repeat(value),
                        selected = star == value,
                        onClick = { star = value },
                    )
                }
            }

            Text("尺寸（短边像素）", color = HawkColors.Fg1, fontSize = 12.sp)
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                SideField(
                    placeholder = "最小",
                    value = minSide,
                    onValueChange = { minSide = it },
                    modifier = Modifier.weight(1f),
                )
                Text("–", color = HawkColors.Fg1)
                SideField(
                    placeholder = "最大",
                    value = maxSide,
                    onValueChange = { maxSide = it },
                    modifier = Modifier.weight(1f),
                )
            }
            Text("下限", color = HawkColors.Fg1, fontSize = 11.sp)
            TierRow { minSide = it.toString() }
            Text("上限", color = HawkColors.Fg1, fontSize = 11.sp)
            TierRow { maxSide = it.toString() }
            Text(
                text = "横图与竖图都按短边比较；留空表示不限。",
                color = HawkColors.Fg1,
                fontSize = 11.sp,
            )
            sizeError?.let { Text(text = it, color = HawkColors.Danger, fontSize = 11.sp) }

            Text("颜色", color = HawkColors.Fg1, fontSize = 12.sp)
            FlowRow(
                horizontalArrangement = Arrangement.spacedBy(12.dp),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                HawkColors.FilterColors.forEach { hex ->
                    val selected = color == hex
                    Column(
                        horizontalAlignment = Alignment.CenterHorizontally,
                        modifier = Modifier
                            .clip(CircleShape)
                            .clickable { color = if (selected) null else hex },
                    ) {
                        androidx.compose.foundation.layout.Box(
                            modifier = Modifier
                                .size(34.dp)
                                .clip(CircleShape)
                                .background(Color.fromHex(hex))
                                .then(
                                    if (selected) {
                                        Modifier.background(
                                            HawkColors.Accent.copy(alpha = 0.25f),
                                            CircleShape,
                                        )
                                    } else {
                                        Modifier
                                    }
                                ),
                        )
                    }
                }
            }
            if (color != null) {
                TextButton(onClick = { color = null }) {
                    Text("清除颜色筛选（$color）", color = HawkColors.Danger, fontSize = 12.sp)
                }
            }
            Text(
                text = "按素材调色板近似匹配，与桌面端颜色检索一致。",
                color = HawkColors.Fg1,
                fontSize = 11.sp,
            )

                Button(
                    onClick = { apply() },
                    modifier = Modifier.fillMaxWidth().height(44.dp),
                    shape = RoundedCornerShape(8.dp),
                ) {
                    Text("应用")
                }
            }
        }
    }
}

@Composable
private fun SideField(
    placeholder: String,
    value: String,
    onValueChange: (String) -> Unit,
    modifier: Modifier = Modifier,
) {
    OutlinedTextField(
        value = value,
        onValueChange = { input ->
            if (input.all { it.isDigit() } && input.length <= 5) onValueChange(input)
        },
        placeholder = { Text(placeholder, color = HawkColors.Fg1) },
        singleLine = true,
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
        modifier = modifier,
    )
}

@Composable
private fun TierRow(fill: (Int) -> Unit) {
    FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        TIERS.forEach { value ->
            Text(
                text = "${value}p",
                color = HawkColors.Fg0,
                fontSize = 11.sp,
                modifier = Modifier
                    .clip(RoundedCornerShape(50))
                    .background(HawkColors.Bg3)
                    .clickable { fill(value) }
                    .padding(horizontal = 10.dp, vertical = 5.dp),
            )
        }
    }
}

@Composable
private fun FilterChip(label: String, selected: Boolean, onClick: () -> Unit) {
    Text(
        text = label,
        color = if (selected) HawkColors.Accent else HawkColors.Fg1,
        fontSize = 12.sp,
        fontWeight = if (selected) FontWeight.SemiBold else FontWeight.Normal,
        modifier = Modifier
            .clip(RoundedCornerShape(50))
            .background(if (selected) HawkColors.Accent.copy(alpha = 0.15f) else HawkColors.Bg3)
            .clickable(onClick = onClick)
            .padding(horizontal = 11.dp, vertical = 6.dp),
    )
}

private fun parseSide(text: String): Int? {
    val trimmed = text.trim()
    if (trimmed.isEmpty()) return null
    val value = trimmed.toIntOrNull()
    return if (value == null || value <= 0 || value > 99_999) {
        null
    } else {
        value
    }
}

