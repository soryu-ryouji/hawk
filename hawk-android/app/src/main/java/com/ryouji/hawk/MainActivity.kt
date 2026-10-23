package com.ryouji.hawk

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.viewModels
import com.ryouji.hawk.ui.RootScreen
import com.ryouji.hawk.ui.theme.HawkTheme

class MainActivity : ComponentActivity() {
    private val viewModel: AppViewModel by viewModels()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        if (BuildConfig.DEBUG) {
            viewModel.seedDebugProfileIfAvailable()
        }
        setContent {
            HawkTheme {
                RootScreen(viewModel)
            }
        }
    }
}
