package com.ryouji.hawk.ui.connect

import android.content.ClipboardManager
import androidx.compose.ui.platform.LocalContext

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.ChevronRight
import androidx.compose.material.icons.outlined.ContentPaste
import androidx.compose.material.icons.outlined.Delete
import androidx.compose.material.icons.outlined.Edit
import androidx.compose.material.icons.outlined.MoreVert
import androidx.compose.material.icons.outlined.Public
import androidx.compose.material.icons.outlined.QrCodeScanner
import androidx.compose.material.icons.outlined.Wifi
import androidx.compose.material3.Button
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.ryouji.hawk.AppViewModel
import com.ryouji.hawk.data.connection.ConnectionException
import com.ryouji.hawk.data.connection.ConnectionKind
import com.ryouji.hawk.data.connection.ConnectionProfile
import com.ryouji.hawk.data.connection.PairingPayload
import com.ryouji.hawk.ui.HawkMark
import com.ryouji.hawk.ui.theme.HawkColors

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ConnectScreen(viewModel: AppViewModel) {
    val appState by viewModel.state.collectAsStateWithLifecycle()
    val bookmark by viewModel.connections.bookmark.collectAsStateWithLifecycle()

    var formTarget by remember { mutableStateOf<ProfileFormTarget?>(null) }
    var showScanner by remember { mutableStateOf(false) }
    var scanError by remember { mutableStateOf<String?>(null) }
    val context = LocalContext.current
    val clipboard = context.getSystemService(ClipboardManager::class.java)

    fun handlePairingCode(code: String) {
        try {
            viewModel.saveAndConnect(PairingPayload.decode(code).toProfile())
        } catch (error: ConnectionException) {
            scanError = error.message
        }
    }

    fun importClipboard() {
        val text = clipboard?.primaryClip?.getItemAt(0)?.text?.toString()?.trim().orEmpty()
        if (text.isEmpty()) {
            scanError = "剪贴板中没有配对信息"
        } else {
            handlePairingCode(text)
        }
    }

    Scaffold(
        containerColor = HawkColors.Bg0,
        topBar = {
            TopAppBar(
                title = { Text("hawk", color = HawkColors.Fg0) },
                colors = TopAppBarDefaults.topAppBarColors(containerColor = HawkColors.Bg0),
                actions = {
                    IconButton(onClick = { showScanner = true }) {
                        Icon(
                            Icons.Outlined.QrCodeScanner,
                            contentDescription = "扫描配对二维码",
                            tint = HawkColors.Fg0,
                        )
                    }
                },
            )
        },
    ) { padding ->
        Box(
            modifier = Modifier.fillMaxSize().padding(padding),
            contentAlignment = Alignment.TopCenter,
        ) {
            Column(
                modifier = Modifier
                    .widthIn(max = 620.dp)
                    .fillMaxWidth()
                    .verticalScroll(rememberScrollState())
                    .padding(horizontal = 20.dp)
                    .padding(bottom = 32.dp),
                verticalArrangement = Arrangement.spacedBy(20.dp),
            ) {
                Header()
                appState.lastError?.let { error ->
                    ErrorBanner(message = error, onDismiss = viewModel::clearLastError)
                }
                ConnectionActions(
                    onScan = { showScanner = true },
                    onPaste = ::importClipboard,
                    onLan = { formTarget = ProfileFormTarget(ConnectionKind.Lan, null) },
                    onRemote = { formTarget = ProfileFormTarget(ConnectionKind.Remote, null) },
                )
                ProfileList(
                    profiles = bookmark.profiles,
                    activeId = bookmark.activeId,
                    onConnect = viewModel::connect,
                    onEdit = { formTarget = ProfileFormTarget(it.kind, it) },
                    onDelete = viewModel::deleteProfile,
                )
            }
        }
    }

    formTarget?.let { target ->
        ProfileFormDialog(
            kind = target.kind,
            existing = target.profile,
            onDismiss = { formTarget = null },
            onSave = { profile ->
                formTarget = null
                viewModel.saveAndConnect(profile)
            },
        )
    }

    if (showScanner) {
        QrScannerDialog(
            onDismiss = { showScanner = false },
            onCode = { code ->
                showScanner = false
                handlePairingCode(code)
            },
        )
    }

    scanError?.let { message ->
        androidx.compose.material3.AlertDialog(
            onDismissRequest = { scanError = null },
            confirmButton = {
                androidx.compose.material3.TextButton(onClick = { scanError = null }) {
                    Text("好")
                }
            },
            title = { Text("无法导入配对信息") },
            text = { Text(message) },
            containerColor = HawkColors.Bg2,
        )
    }
}

private data class ProfileFormTarget(
    val kind: ConnectionKind,
    val profile: ConnectionProfile?,
)

@Composable
private fun Header() {
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            HawkMark(size = 58)
            Spacer(modifier = Modifier.width(14.dp))
            Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(
                    text = "hawk",
                    color = HawkColors.Fg0,
                    fontSize = 24.sp,
                    fontWeight = FontWeight.Bold,
                )
                Text(
                    text = "在 Android 手机和平板上浏览你的素材库",
                    color = HawkColors.Fg1,
                    fontSize = 13.sp,
                )
            }
        }
        Text(
            text = "连接同一局域网内的桌面端，或经自托管 hawk-server 远程查看。当前客户端为只读浏览。",
            color = HawkColors.Fg1,
            fontSize = 12.sp,
        )
    }
}

@Composable
private fun ConnectionActions(
    onScan: () -> Unit,
    onPaste: () -> Unit,
    onLan: () -> Unit,
    onRemote: () -> Unit,
) {
    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Button(
            onClick = onScan,
            modifier = Modifier.fillMaxWidth(),
            shape = RoundedCornerShape(10.dp),
        ) {
            Icon(Icons.Outlined.QrCodeScanner, contentDescription = null, modifier = Modifier.size(18.dp))
            Spacer(modifier = Modifier.width(8.dp))
            Text("扫描配对二维码")
        }
        OutlinedButton(
            onClick = onPaste,
            modifier = Modifier.fillMaxWidth(),
            shape = RoundedCornerShape(10.dp),
        ) {
            Icon(Icons.Outlined.ContentPaste, contentDescription = null, modifier = Modifier.size(18.dp))
            Spacer(modifier = Modifier.width(8.dp))
            Text("从剪贴板导入配对信息")
        }
        Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            OutlinedButton(
                onClick = onLan,
                modifier = Modifier.weight(1f),
                shape = RoundedCornerShape(10.dp),
            ) {
                Icon(Icons.Outlined.Wifi, contentDescription = null, modifier = Modifier.size(16.dp))
                Spacer(modifier = Modifier.width(6.dp))
                Text("局域网")
            }
            OutlinedButton(
                onClick = onRemote,
                modifier = Modifier.weight(1f),
                shape = RoundedCornerShape(10.dp),
            ) {
                Icon(Icons.Outlined.Public, contentDescription = null, modifier = Modifier.size(16.dp))
                Spacer(modifier = Modifier.width(6.dp))
                Text("广域网")
            }
        }
        Text(
            text = "桌面端生成二维码后直接扫码连接；也可以手动填写地址与凭证。",
            color = HawkColors.Fg1,
            fontSize = 11.sp,
        )
    }
}

@Composable
private fun ProfileList(
    profiles: List<ConnectionProfile>,
    activeId: String?,
    onConnect: (ConnectionProfile) -> Unit,
    onEdit: (ConnectionProfile) -> Unit,
    onDelete: (ConnectionProfile) -> Unit,
) {
    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Text(
            text = "已保存的连接",
            color = HawkColors.Fg1,
            fontSize = 12.sp,
            fontWeight = FontWeight.SemiBold,
        )
        if (profiles.isEmpty()) {
            Text(
                text = "还没有连接。局域网连接需要桌面端「设置 → 局域网」里的地址和 token；" +
                    "广域网连接需要服务器地址、service-key 与 hawk-app key。",
                color = HawkColors.Fg1,
                fontSize = 12.sp,
                modifier = Modifier
                    .fillMaxWidth()
                    .clip(RoundedCornerShape(10.dp))
                    .background(HawkColors.Bg1)
                    .padding(14.dp),
            )
        } else {
            profiles.forEach { profile ->
                ProfileRow(
                    profile = profile,
                    active = activeId == profile.id,
                    onConnect = { onConnect(profile) },
                    onEdit = { onEdit(profile) },
                    onDelete = { onDelete(profile) },
                )
            }
        }
    }
}

@Composable
private fun ProfileRow(
    profile: ConnectionProfile,
    active: Boolean,
    onConnect: () -> Unit,
    onEdit: () -> Unit,
    onDelete: () -> Unit,
) {
    var menuOpen by remember { mutableStateOf(false) }
    Surface(
        color = HawkColors.Bg1,
        shape = RoundedCornerShape(10.dp),
        border = androidx.compose.foundation.BorderStroke(
            1.dp,
            if (active) HawkColors.Accent.copy(alpha = 0.55f) else HawkColors.Border,
        ),
        modifier = Modifier.fillMaxWidth().clickable(onClick = onConnect),
    ) {
        Row(
            modifier = Modifier.padding(12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(
                modifier = Modifier
                    .size(30.dp)
                    .clip(RoundedCornerShape(8.dp))
                    .background(HawkColors.Bg3),
                contentAlignment = Alignment.Center,
            ) {
                Icon(
                    imageVector = iconFor(profile.kind),
                    contentDescription = null,
                    tint = HawkColors.Accent,
                    modifier = Modifier.size(16.dp),
                )
            }
            Spacer(modifier = Modifier.width(12.dp))
            Column(modifier = Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(
                        text = profile.displayName,
                        color = HawkColors.Fg0,
                        fontSize = 14.sp,
                        fontWeight = FontWeight.Medium,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    if (active) {
                        Spacer(modifier = Modifier.width(6.dp))
                        Text(
                            text = "上次使用",
                            color = HawkColors.Accent,
                            fontSize = 10.sp,
                            modifier = Modifier
                                .clip(CircleShape)
                                .background(HawkColors.Accent.copy(alpha = 0.14f))
                                .padding(horizontal = 5.dp, vertical = 1.dp),
                        )
                    }
                }
                Text(
                    text = "${profile.kind.title} · ${profile.url}",
                    color = HawkColors.Fg1,
                    fontSize = 11.sp,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            Icon(
                Icons.Outlined.ChevronRight,
                contentDescription = null,
                tint = HawkColors.Fg1,
                modifier = Modifier.size(16.dp),
            )
            Box {
                IconButton(onClick = { menuOpen = true }, modifier = Modifier.size(30.dp)) {
                    Icon(
                        Icons.Outlined.MoreVert,
                        contentDescription = "连接操作",
                        tint = HawkColors.Fg1,
                        modifier = Modifier.size(18.dp),
                    )
                }
                DropdownMenu(
                    expanded = menuOpen,
                    onDismissRequest = { menuOpen = false },
                    containerColor = HawkColors.Bg2,
                ) {
                    DropdownMenuItem(
                        text = { Text("编辑") },
                        leadingIcon = { Icon(Icons.Outlined.Edit, contentDescription = null) },
                        onClick = {
                            menuOpen = false
                            onEdit()
                        },
                    )
                    DropdownMenuItem(
                        text = { Text("删除", color = HawkColors.Danger) },
                        leadingIcon = {
                            Icon(
                                Icons.Outlined.Delete,
                                contentDescription = null,
                                tint = HawkColors.Danger,
                            )
                        },
                        onClick = {
                            menuOpen = false
                            onDelete()
                        },
                    )
                }
            }
        }
    }
}

private fun iconFor(kind: ConnectionKind): ImageVector =
    if (kind == ConnectionKind.Lan) Icons.Outlined.Wifi else Icons.Outlined.Public

@Composable
fun ErrorBanner(message: String, onDismiss: (() -> Unit)? = null) {
    Surface(
        color = HawkColors.Danger.copy(alpha = 0.12f),
        shape = RoundedCornerShape(8.dp),
        border = androidx.compose.foundation.BorderStroke(
            1.dp,
            HawkColors.Danger.copy(alpha = 0.35f),
        ),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Row(
            modifier = Modifier.padding(12.dp),
            verticalAlignment = Alignment.Top,
        ) {
            Text(text = "!", color = HawkColors.Danger, fontWeight = FontWeight.Bold)
            Spacer(modifier = Modifier.width(8.dp))
            Text(
                text = message,
                color = HawkColors.Fg0,
                fontSize = 12.sp,
                modifier = Modifier.weight(1f),
            )
            if (onDismiss != null) {
                IconButton(onClick = onDismiss, modifier = Modifier.size(20.dp)) {
                    Text("×", color = HawkColors.Fg1, fontSize = 16.sp)
                }
            }
        }
    }
}

@Composable
fun SectionHint(text: String) {
    Text(
        text = text,
        color = HawkColors.Fg1,
        fontSize = 11.sp,
        modifier = Modifier.padding(horizontal = 14.dp, vertical = 4.dp),
    )
}
