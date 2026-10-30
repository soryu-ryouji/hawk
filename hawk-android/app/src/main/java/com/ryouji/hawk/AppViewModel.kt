package com.ryouji.hawk

import android.app.Application
import androidx.core.content.edit
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import com.ryouji.hawk.data.connection.ConnectionKind
import com.ryouji.hawk.data.connection.ConnectionLauncher
import com.ryouji.hawk.data.connection.ConnectionProfile
import com.ryouji.hawk.data.connection.ConnectionStore
import com.ryouji.hawk.data.model.HawkJson
import com.ryouji.hawk.data.library.LibraryStore
import com.ryouji.hawk.data.net.ApiClient
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

sealed interface AppPhase {
    data object Launching : AppPhase
    data object Connect : AppPhase
    data class Connecting(val message: String) : AppPhase
    data object Ready : AppPhase
}

class LibrarySession(
    val api: ApiClient,
    val profile: ConnectionProfile,
    val label: String,
    val role: String,
    val targetDeviceName: String?,
    val targetLibraryName: String?,
    val store: LibraryStore,
) {
    val isRemote: Boolean get() = profile.kind == ConnectionKind.Remote

    val accessLabel: String
        get() = when (role) {
            "admin" -> "完全访问"
            "editor" -> "可读写"
            "importer" -> "仅导入"
            else -> "只读"
        }
}

data class AppUiState(
    val phase: AppPhase = AppPhase.Launching,
    val session: LibrarySession? = null,
    val lastError: String? = null,
)

class AppViewModel(application: Application) : AndroidViewModel(application) {
    private val prefs = application.getSharedPreferences("hawk.android", Application.MODE_PRIVATE)
    val connections = ConnectionStore(application)

    private val _state = MutableStateFlow(AppUiState())
    val state: StateFlow<AppUiState> = _state.asStateFlow()

    private val defaultThumbSize = if (application.resources.configuration.smallestScreenWidthDp >= 600) 240f else 150f
    private val _thumbSize = MutableStateFlow(prefs.getFloat(KEY_THUMB_SIZE, defaultThumbSize))
    val thumbSize: StateFlow<Float> = _thumbSize.asStateFlow()

    private var connectJob: Job? = null
    private var sessionScope: CoroutineScope? = null
    private var started = false

    fun start() {
        if (started) return
        started = true
        val profile = connections.activeProfile
        if (profile != null) {
            connect(profile)
        } else {
            setState(AppPhase.Connect, null, null)
        }
    }

    fun connect(profile: ConnectionProfile) {
        connectJob?.cancel()
        teardownRemoteSession()
        sessionScope?.cancel()
        sessionScope = null
        _state.value = AppUiState(AppPhase.Connecting("正在连接 ${profile.displayName}…"))

        connectJob = viewModelScope.launch {
            var scope: CoroutineScope? = null
            try {
                val outcome = ConnectionLauncher.launch(profile)
                ensureActive()
                val saved = connections.upsert(profile)
                connections.setActive(saved.id)

                scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
                sessionScope = scope
                val store = LibraryStore(outcome.api, scope)
                store.bootstrap()
                ensureActive()

                val label = outcome.targetLibraryName?.takeIf { it.isNotBlank() }?.let {
                    "${outcome.label} · $it"
                } ?: store.state.value.library?.name ?: outcome.label
                val session = LibrarySession(
                    api = outcome.api,
                    profile = saved,
                    label = label,
                    role = outcome.info.role,
                    targetDeviceName = outcome.targetDeviceName,
                    targetLibraryName = outcome.targetLibraryName,
                    store = store,
                )
                _state.value = AppUiState(AppPhase.Ready, session, null)
            } catch (error: CancellationException) {
                scope?.cancel()
                throw error
            } catch (error: Exception) {
                scope?.cancel()
                sessionScope = null
                _state.value = AppUiState(AppPhase.Connect, null, errorMessage(error))
            }
        }
    }

    fun saveAndConnect(profile: ConnectionProfile) {
        val saved = connections.upsert(profile)
        connect(saved)
    }

    fun reconnect() {
        val profile = _state.value.session?.profile ?: connections.activeProfile
        if (profile != null) connect(profile)
    }

    fun disconnect() {
        connectJob?.cancel()
        teardownRemoteSession()
        sessionScope?.cancel()
        sessionScope = null
        _state.value = AppUiState(AppPhase.Connect, null, null)
    }

    fun showConnectScreen() {
        disconnect()
    }

    fun deleteProfile(profile: ConnectionProfile) {
        connections.remove(profile.id)
        if (_state.value.session?.profile?.id == profile.id) disconnect()
    }

    fun setThumbSize(value: Float) {
        _thumbSize.value = value
        prefs.edit { putFloat(KEY_THUMB_SIZE, value) }
    }

    fun clearLastError() {
        _state.value = _state.value.copy(lastError = null)
    }

    /**
     * Debug 冒烟：把 `ConnectionProfile` JSON 推到应用外部私有目录
     * （`/sdcard/Android/data/<包名>/files/hawk_profile.json`）后启动即可预置连接，
     * 免去自动化测试里手点表单。仅 debug 构建、且本地没有任何连接时生效。
     */
    fun seedDebugProfileIfAvailable() {
        if (!BuildConfig.DEBUG || connections.profiles.isNotEmpty()) return
        val app = getApplication<Application>()
        val file = app.getExternalFilesDir(null)?.resolve("hawk_profile.json") ?: return
        if (!file.isFile) return
        val profile = runCatching {
            HawkJson.decodeFromString(ConnectionProfile.serializer(), file.readText())
        }.getOrNull() ?: return
        val saved = connections.upsert(profile)
        connections.setActive(saved.id)
    }

    /** 显式断开/切换时通知服务端注销查看会话（best-effort；失败不阻塞 UI）。 */
    private fun teardownRemoteSession() {
        val session = _state.value.session ?: return
        if (!session.isRemote) return
        viewModelScope.launch {
            runCatching { session.api.delete("/api/v1/viewer/session") }
        }
    }

    private fun errorMessage(error: Throwable): String =
        error.message?.takeIf { it.isNotBlank() } ?: "无法连接素材库服务"

    override fun onCleared() {
        sessionScope?.cancel()
    }

    private fun setState(phase: AppPhase, session: LibrarySession?, lastError: String?) {
        _state.value = AppUiState(phase = phase, session = session, lastError = lastError)
    }

    private companion object {
        const val KEY_THUMB_SIZE = "thumb_size"
    }
}
