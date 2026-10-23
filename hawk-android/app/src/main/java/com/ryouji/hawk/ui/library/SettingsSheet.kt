package com.ryouji.hawk.ui.library

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Slider
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
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
    val store = session.store
    val state by store.state.collectAsStateWithLifecycle()
    val thumbSize by viewModel.thumbSize.collectAsStateWithLifecycle()

    ModalBottomSheet(onDismissRequest = onDismiss, containerColor = HawkColors.Bg1) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 20.dp)
                .padding(bottom = 28.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(
                text = "连接信息",
                color = HawkColors.Fg0,
                fontSize = 16.sp,
                fontWeight = FontWeight.SemiBold,
            )

            InfoRow("素材库", state.library?.name ?: session.label)
            InfoRow(
                "连接方式",
                if (session.isRemote) "广域网（hawk-server）" else "局域网直连",
            )
            InfoRow("访问级别", session.accessLabel)
            session.targetDeviceName?.takeIf { it.isNotBlank() }?.let {
                InfoRow("目标设备", it)
            }
            InfoRow("服务地址", session.profile.url)
            InfoRow("全部条目", if (state.countsLoading) "…" else state.counts.all.toString())
            InfoRow("当前视图", "${state.total} 项 · ${Format.bytes(state.totalSize)}")

            Text(
                text = "Android 客户端当前为只读浏览；导入、编辑与删除请在桌面端操作。",
                color = HawkColors.Fg1,
                fontSize = 11.sp,
            )

            HorizontalDivider(color = HawkColors.Border)

            Text(
                text = "连接操作",
                color = HawkColors.Fg1,
                fontSize = 12.sp,
                fontWeight = FontWeight.SemiBold,
            )
            Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                TextButton(onClick = {
                    viewModel.reconnect()
                    onDismiss()
                }) { Text("重新连接", color = HawkColors.Accent) }
                TextButton(onClick = {
                    viewModel.showConnectScreen()
                    onDismiss()
                }) { Text("切换连接", color = HawkColors.Fg1) }
                TextButton(onClick = {
                    viewModel.disconnect()
                    onDismiss()
                }) { Text("断开连接", color = HawkColors.Danger) }
            }

            HorizontalDivider(color = HawkColors.Border)

            Text(
                text = "外观",
                color = HawkColors.Fg1,
                fontSize = 12.sp,
                fontWeight = FontWeight.SemiBold,
            )
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
            Text(
                text = "贴近桌面端的齐行网格：数值是每行目标高度，不再是固定列数。",
                color = HawkColors.Fg1,
                fontSize = 11.sp,
            )

            HorizontalDivider(color = HawkColors.Border)

            Text(
                text = "关于",
                color = HawkColors.Fg1,
                fontSize = 12.sp,
                fontWeight = FontWeight.SemiBold,
            )
            InfoRow("hawk Android", "1.0")
            state.library?.applicationVersion?.let { InfoRow("桌面端版本", it) }
            state.library?.storageMode?.let { InfoRow("元数据存储", it) }
        }
    }
}

@Composable
private fun InfoRow(label: String, value: String) {
    Row(
        modifier = Modifier.fillMaxWidth(),
        verticalAlignment = Alignment.Top,
    ) {
        Text(text = label, color = HawkColors.Fg1, fontSize = 12.sp, modifier = Modifier.weight(1f))
        Text(text = value, color = HawkColors.Fg0, fontSize = 12.sp)
    }
}
