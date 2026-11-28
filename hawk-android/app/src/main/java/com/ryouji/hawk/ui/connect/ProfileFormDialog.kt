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
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import com.ryouji.hawk.data.connection.ConnectionProfile
import com.ryouji.hawk.ui.theme.HawkColors

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ProfileFormDialog(
    existing: ConnectionProfile?,
    onDismiss: () -> Unit,
    onSave: (ConnectionProfile) -> Unit,
) {
    var name by remember { mutableStateOf(existing?.name.orEmpty()) }
    var url by remember { mutableStateOf(existing?.url.orEmpty()) }
    var accessKey by remember { mutableStateOf(existing?.accessKey.orEmpty()) }
    var error by remember { mutableStateOf<String?>(null) }

    val canSave = url.trim().isNotEmpty() && accessKey.trim().isNotEmpty()

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
                            text = if (existing == null) "添加局域网连接" else "编辑连接",
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
                            val saved = buildProfile(existing, name, url, accessKey)
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
                    placeholder = { Text("192.168.1.5:27375", color = HawkColors.Fg1) },
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
                    text = "桌面端：设置 → 局域网网站访问 → 创建访问密钥；密钥明文只在创建时显示。",
                    color = HawkColors.Fg1,
                    fontSize = 11.sp,
                )

                Text("访问密钥", color = HawkColors.Fg1, fontSize = 12.sp)
                OutlinedTextField(
                    value = accessKey,
                    onValueChange = { accessKey = it },
                    placeholder = { Text("hk_…", color = HawkColors.Fg1) },
                    singleLine = true,
                    visualTransformation = PasswordVisualTransformation(),
                    keyboardOptions = KeyboardOptions(
                        keyboardType = KeyboardType.Password,
                        imeAction = ImeAction.Done,
                    ),
                    modifier = Modifier.fillMaxWidth(),
                )

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

private fun buildProfile(
    existing: ConnectionProfile?,
    name: String,
    url: String,
    accessKey: String,
): ConnectionProfile {
    val base = existing ?: ConnectionProfile(url = url.trim())
    return base.copy(
        name = name.trim(),
        url = url.trim(),
        accessKey = accessKey.trim(),
    )
}
