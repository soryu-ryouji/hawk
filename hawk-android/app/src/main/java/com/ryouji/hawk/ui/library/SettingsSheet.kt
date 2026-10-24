package com.ryouji.hawk.ui.library

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Close
import androidx.compose.material.icons.outlined.Refresh
import androidx.compose.material.icons.outlined.SwapHoriz
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Slider
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.ryouji.hawk.AppViewModel
import com.ryouji.hawk.LibrarySession
import com.ryouji.hawk.ui.theme.HawkColors
import com.ryouji.hawk.util.Format

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SettingsSheet(
    viewModel: AppViewModel,
    session: LibrarySession,
    onDismiss: () -> Unit,
) {
    val state by session.store.state.collectAsStateWithLifecycle()
    val thumbSize by viewModel.thumbSize.collectAsStateWithLifecycle()

    ModalBottomSheet(onDismissRequest = onDismiss, containerColor = HawkColors.Bg1) {
        Box(modifier = Modifier.fillMaxWidth(), contentAlignment = Alignment.TopCenter) {
            Column(
                modifier = Modifier
                    .widthIn(max = 560.dp)
                    .fillMaxWidth()
                    .verticalScroll(rememberScrollState())
                    .padding(horizontal = 18.dp)
                    .padding(bottom = 28.dp),
                verticalArrangement = Arrangement.spacedBy(18.dp),
            ) {
                Text(
                    text = "连接信息",
                    color = HawkColors.Fg0,
                    fontSize = 17.sp,
                    fontWeight = FontWeight.SemiBold,
                )

                SettingsSection("当前连接") {
                    SettingsInfoRow("素材库", state.library?.name ?: session.label)
                    SettingsDivider()
                    SettingsInfoRow("连接方式", if (session.isRemote) "广域网" else "局域网直连")
                    SettingsDivider()
                    SettingsInfoRow("访问级别", session.accessLabel)
                    session.targetDeviceName?.takeIf { it.isNotBlank() }?.let {
                        SettingsDivider()
                        SettingsInfoRow("目标设备", it)
                    }
                    SettingsDivider()
                    SettingsInfoRow("服务地址", session.profile.url)
                    SettingsDivider()
                    SettingsInfoRow("素材数量", if (state.countsLoading) "…" else "${state.counts.all} 项")
                    SettingsDivider()
                    SettingsInfoRow("当前视图", "${state.total} 项 · ${Format.bytes(state.totalSize)}")
                    SettingsDivider()
                    Text(
                        text = "Android 客户端为只读浏览；导入、编辑与删除请在桌面端操作。",
                        color = HawkColors.Fg1,
                        fontSize = 11.sp,
                        lineHeight = 16.sp,
                        modifier = Modifier.padding(horizontal = 12.dp, vertical = 10.dp),
                    )
                }

                SettingsSection("连接操作") {
                    SettingsActionRow("重新连接", Icons.Outlined.Refresh) {
                        viewModel.reconnect()
                        onDismiss()
                    }
                    SettingsDivider()
                    SettingsActionRow("切换素材库", Icons.Outlined.SwapHoriz) {
                        viewModel.showConnectScreen()
                        onDismiss()
                    }
                    SettingsDivider()
                    SettingsActionRow("断开连接", Icons.Outlined.Close, destructive = true) {
                        viewModel.disconnect()
                        onDismiss()
                    }
                }

                SettingsSection("外观") {
                    Column(
                        modifier = Modifier.padding(horizontal = 12.dp, vertical = 10.dp),
                        verticalArrangement = Arrangement.spacedBy(4.dp),
                    ) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Text(
                                text = "缩略图尺寸",
                                color = HawkColors.Fg0,
                                fontSize = 13.sp,
                                modifier = Modifier.weight(1f),
                            )
                            Text(
                                text = thumbSize.toInt().toString(),
                                color = HawkColors.Fg1,
                                fontSize = 12.sp,
                            )
                        }
                        Slider(
                            value = thumbSize,
                            onValueChange = viewModel::setThumbSize,
                            valueRange = 120f..280f,
                            steps = 19,
                        )
                    }
                    SettingsDivider()
                    Text(
                        text = "数值是每行目标高度：网格按图片比例齐行，越大看得越清楚，越小一屏放得越多。",
                        color = HawkColors.Fg1,
                        fontSize = 11.sp,
                        lineHeight = 16.sp,
                        modifier = Modifier.padding(horizontal = 12.dp, vertical = 10.dp),
                    )
                }

                SettingsSection("关于") {
                    SettingsInfoRow("hawk Android", "1.0")
                    state.library?.applicationVersion?.let {
                        SettingsDivider()
                        SettingsInfoRow("桌面端版本", it)
                    }
                    state.library?.storageMode?.let {
                        SettingsDivider()
                        SettingsInfoRow("元数据存储", it)
                    }
                }
            }
        }
    }
}

@Composable
private fun SettingsSection(
    title: String,
    content: @Composable ColumnScope.() -> Unit,
) {
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Text(
            text = title,
            color = HawkColors.Fg1,
            fontSize = 11.sp,
            fontWeight = FontWeight.SemiBold,
            modifier = Modifier.padding(horizontal = 2.dp),
        )
        Surface(
            color = HawkColors.Bg2,
            shape = RoundedCornerShape(12.dp),
            modifier = Modifier.fillMaxWidth(),
        ) {
            Column(content = content)
        }
    }
}

@Composable
private fun SettingsInfoRow(label: String, value: String) {
    Row(
        modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 10.dp),
        verticalAlignment = Alignment.Top,
    ) {
        Text(
            text = label,
            color = HawkColors.Fg1,
            fontSize = 12.sp,
            modifier = Modifier.weight(1f),
        )
        Text(
            text = value,
            color = HawkColors.Fg0,
            fontSize = 12.sp,
            modifier = Modifier.weight(1.4f),
        )
    }
}

@Composable
private fun SettingsActionRow(
    label: String,
    icon: ImageVector,
    destructive: Boolean = false,
    onClick: () -> Unit,
) {
    val tint = if (destructive) HawkColors.Danger else HawkColors.Fg0
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick)
            .padding(horizontal = 12.dp, vertical = 11.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Icon(
            imageVector = icon,
            contentDescription = null,
            tint = if (destructive) HawkColors.Danger else HawkColors.Fg1,
            modifier = Modifier.size(17.dp),
        )
        Text(text = label, color = tint, fontSize = 13.sp)
        Spacer(modifier = Modifier.weight(1f))
    }
}

@Composable
private fun SettingsDivider() {
    HorizontalDivider(color = HawkColors.Border)
}