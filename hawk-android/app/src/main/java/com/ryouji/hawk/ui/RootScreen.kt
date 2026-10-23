package com.ryouji.hawk.ui

import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.ryouji.hawk.AppPhase
import com.ryouji.hawk.AppViewModel
import com.ryouji.hawk.R
import com.ryouji.hawk.ui.connect.ConnectScreen
import com.ryouji.hawk.ui.library.LibraryScreen
import com.ryouji.hawk.ui.theme.HawkColors

@Composable
fun RootScreen(viewModel: AppViewModel) {
    val state by viewModel.state.collectAsStateWithLifecycle()
    LaunchedEffect(Unit) { viewModel.start() }

    when (val phase = state.phase) {
        AppPhase.Launching -> LaunchScreen()
        AppPhase.Connect -> ConnectScreen(viewModel)
        is AppPhase.Connecting -> ConnectingScreen(
            message = phase.message,
            onCancel = viewModel::disconnect,
        )

        AppPhase.Ready -> {
            val session = state.session
            if (session != null) {
                LibraryScreen(viewModel, session)
            } else {
                ConnectScreen(viewModel)
            }
        }
    }
}

@Composable
fun HawkMark(size: Int = 58) {
    Box(
        modifier = Modifier
            .size(size.dp)
            .clip(RoundedCornerShape((size * 0.24f).dp))
            .background(HawkColors.Bg3)
    ) {
        Image(
            painter = painterResource(R.drawable.hawk_mark),
            contentDescription = null,
            modifier = Modifier.fillMaxSize(),
        )
    }
}

@Composable
private fun LaunchScreen() {
    Column(
        modifier = Modifier.fillMaxSize().background(HawkColors.Bg0),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.Center,
    ) {
        HawkMark(size = 88)
        Text(
            text = "hawk",
            color = HawkColors.Fg0,
            fontSize = 22.sp,
            fontWeight = FontWeight.SemiBold,
            modifier = Modifier.padding(top = 18.dp),
        )
        CircularProgressIndicator(
            modifier = Modifier.padding(top = 16.dp).size(28.dp),
            color = HawkColors.Fg1,
            strokeWidth = 2.dp,
        )
    }
}

@Composable
private fun ConnectingScreen(message: String, onCancel: () -> Unit) {
    Column(
        modifier = Modifier.fillMaxSize().background(HawkColors.Bg0).padding(28.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.Center,
    ) {
        HawkMark(size = 76)
        Text(
            text = message,
            color = HawkColors.Fg0,
            fontSize = 15.sp,
            fontWeight = FontWeight.Medium,
            modifier = Modifier.padding(top = 20.dp),
        )
        Text(
            text = "正在读取素材库信息；进入后会先显示首屏，其余内容在后台补齐",
            color = HawkColors.Fg1,
            fontSize = 12.sp,
            modifier = Modifier.padding(top = 8.dp),
        )
        CircularProgressIndicator(
            modifier = Modifier.padding(top = 20.dp).size(26.dp),
            color = HawkColors.Accent,
            strokeWidth = 2.dp,
        )
        TextButton(onClick = onCancel, modifier = Modifier.padding(top = 8.dp)) {
            Text("取消", color = HawkColors.Fg1, style = MaterialTheme.typography.bodyMedium)
        }
    }
}
