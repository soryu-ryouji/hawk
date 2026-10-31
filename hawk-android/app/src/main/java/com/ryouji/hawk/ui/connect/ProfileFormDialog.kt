package com.ryouji.hawk.ui.connect

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import com.ryouji.hawk.data.connection.ConnectionKind
import com.ryouji.hawk.data.connection.ConnectionProfile
import com.ryouji.hawk.ui.theme.HawkColors
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ProfileFormDialog(
    kind: ConnectionKind,
    existing: ConnectionProfile?,
    onDismiss: () -> Unit,
    onSave: (ConnectionProfile) -> Unit,
) {
    var name by remember { mutableStateOf(existing?.name.orEmpty()) }
    var url by remember { mutableStateOf(existing?.url.orEmpty()) }
    var token by remember { mutableStateOf(existing?.token.orEmpty()) }
    var targetIdentity by remember { mutableStateOf(existing?.targetIdentity.orEmpty()) }
    var appKey by remember { mutableStateOf(existing?.appKey.orEmpty()) }
    var error by remember { mutableStateOf<String?>(null) }

    val canSave = url.trim().isNotEmpty() && when (kind) {
        ConnectionKind.Lan -> token.trim().isNotEmpty()
        ConnectionKind.Remote -> targetIdentity.trim().isNotEmpty() && appKey.trim().isNotEmpty()
    }

    Dialog(
        onDismissRequest = onDismiss,
        properties = DialogProperties(usePlatformDefaultWidth = false),
    ) {
        Scaffold(
            containerColor = HawkColors.Bg0,
            topBar = {
                TopAppBar(
                    title = {
                        Text(
                            text = if (existing == null) "添加${kind.title}连接" else "编辑连接",
                            color = HawkColors.Fg0,
                            fontSize = 16.sp,
                        )
                    },
                    navigationIcon = {
                        TextButton(onClick = onDismiss) {
                            Text("取消", color = HawkColors.Fg1)
                        }
                    },
                    actions = {
                        TextButton(onClick = {
                            error = null
                            val saved = buildProfile(kind, existing, name, url, token, targetIdentity, appKey)
                            try {
                                saved.baseUrl()
                            } catch (exception: Exception) {
                                error = exception.message ?: "服务地址无效"
                                return@TextButton
                            }
                            onSave(saved)
                        }, enabled = canSave) {
                            Text("保存并连接")
                        }
                    },
                    colors = TopAppBarDefaults.topAppBarColors(containerColor = HawkColors.Bg0),
                )
            },
        ) { padding ->
            Column(
                modifier = Modifier
                    .fillMaxSize()
                    .padding(padding)
                    .verticalScroll(rememberScrollState())
                    .padding(20.dp),
                verticalArrangement = Arrangement.spacedBy(14.dp),
            ) {
                Text("服务地址", color = HawkColors.Fg1, fontSize = 12.sp)
                OutlinedTextField(
                    value = url,
                    onValueChange = { url = it },
                    placeholder = {
                        Text(
                            if (kind == ConnectionKind.Lan) "192.168.1.5:27372"
                            else "https://hawk.example.com",
                            color = HawkColors.Fg1,
                        )
                    },
                    singleLine = true,
                    keyboardOptions = KeyboardOptions(
                        keyboardType = KeyboardType.Uri,
                        imeAction = ImeAction.Next,
                    ),
                    modifier = Modifier.fillMaxWidth(),
                )
                OutlinedTextField(
                    value = name,
                    onValueChange = { name = it },
                    placeholder = { Text("显示名称（可选）", color = HawkColors.Fg1) },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
                Text(
                    text = if (kind == ConnectionKind.Lan) {
                        "桌面端：设置 → 局域网 → 开启网页访问，复制监听地址与访问 token。"
                    } else {
                        "hawk-server 的部署地址；target_identity 与 AccessKey 来自配对信息。"
                    },
                    color = HawkColors.Fg1,
                    fontSize = 11.sp,
                )

                Text("访问凭证", color = HawkColors.Fg1, fontSize = 12.sp)
                if (kind == ConnectionKind.Lan) {
                    SecretField(
                        value = token,
                        onValueChange = { token = it },
                        placeholder = "访问 token",
                        imeAction = ImeAction.Done,
                    )
                } else {
                    SecretField(
                        value = targetIdentity,
                        onValueChange = { targetIdentity = it },
                        placeholder = "target_identity（目标 iroh 公钥）",
                        imeAction = ImeAction.Next,
                    )
                    SecretField(
                        value = appKey,
                        onValueChange = { appKey = it },
                        placeholder = "AccessKey（hk_…）",
                        imeAction = ImeAction.Done,
                    )
                }

                error?.let {
                    Text(
                        text = it,
                        color = HawkColors.Danger,
                        fontSize = 12.sp,
                        modifier = Modifier
                            .fillMaxWidth()
                            .background(HawkColors.Danger.copy(alpha = 0.1f))
                            .padding(10.dp),
                    )
                }
            }
        }
    }
}

@Composable
private fun SecretField(
    value: String,
    onValueChange: (String) -> Unit,
    placeholder: String,
    imeAction: ImeAction,
) {
    OutlinedTextField(
        value = value,
        onValueChange = onValueChange,
        placeholder = { Text(placeholder, color = HawkColors.Fg1) },
        singleLine = true,
        visualTransformation = PasswordVisualTransformation(),
        keyboardOptions = KeyboardOptions(
            keyboardType = KeyboardType.Password,
            imeAction = imeAction,
        ),
        modifier = Modifier.fillMaxWidth(),
    )
}

private fun buildProfile(
    kind: ConnectionKind,
    existing: ConnectionProfile?,
    name: String,
    url: String,
    token: String,
    targetIdentity: String,
    appKey: String,
): ConnectionProfile {
    var urlText = url.trim()
    var accessToken = token.trim()
    // 兼容直接粘贴局域网查看器地址（http://ip:port/?token=...）：token 缺省时从查询参数补。
    if (kind == ConnectionKind.Lan && accessToken.isEmpty()) {
        accessToken = urlText.toHttpUrlOrNull()?.queryParameter("token").orEmpty()
    }
    urlText.toHttpUrlOrNull()?.let { parsed ->
        urlText = parsed.newBuilder().query(null).fragment(null).build().toString()
    }
    val base = existing ?: ConnectionProfile(kind = kind, url = urlText)
    return base.copy(
        kind = kind,
        name = name.trim(),
        url = urlText,
        token = accessToken,
        targetIdentity = targetIdentity.trim(),
        appKey = appKey.trim(),
    )
}

/** 让调用方知道 Compose 默认的可视化转换（避免未使用导入告警）。 */
private val defaultTransformation: VisualTransformation = VisualTransformation.None
