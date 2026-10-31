package com.ryouji.hawk.data.connection

import android.content.Context
import com.ryouji.hawk.data.model.AppInfo
import com.ryouji.hawk.data.model.StartupInfo
import com.ryouji.hawk.data.net.ApiClient
import com.ryouji.hawk.data.net.ApiException
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.delay
import okhttp3.HttpUrl.Companion.toHttpUrl

/**
 * 连接启动：局域网直接验证 token；广域网走 gate → login → session，
 * 拿到 bt 后与 daemon API 同构（服务端隧道代理）。
 */
object ConnectionLauncher {
    data class Outcome(
        val api: ApiClient,
        val info: AppInfo,
        val label: String,
        val targetDeviceName: String?,
        val targetLibraryName: String?,
    )

    suspend fun launch(context: Context, profile: ConnectionProfile): Outcome =
        if (profile.targetIdentity.isNotBlank()) {
            when (profile.kind) {
                ConnectionKind.Lan -> launchNativeCandidates(context, profile)
                ConnectionKind.Remote -> launchNative(context, profile, profile.baseUrl())
            }
        } else {
            when (profile.kind) {
                ConnectionKind.Lan -> launchLanCandidates(profile)
                ConnectionKind.Remote -> throw ConnectionException("该连接缺少 target_identity，请重新配对")
            }
        }

    private suspend fun launchNativeCandidates(
        context: Context,
        profile: ConnectionProfile,
    ): Outcome {
        var lastError: Exception? = null
        for (baseUrl in profile.candidateBaseUrls()) {
            try {
                return launchNative(context, profile, baseUrl)
            } catch (error: CancellationException) {
                throw error
            } catch (error: Exception) {
                lastError = error
            }
        }
        throw lastError ?: ConnectionException("没有可用的局域网地址")
    }

    private suspend fun launchNative(
        context: Context,
        profile: ConnectionProfile,
        baseUrl: okhttp3.HttpUrl,
    ): Outcome {
        val connection = RustTransport.connect(
            configDir = context.filesDir.resolve("transport").apply { mkdirs() }.absolutePath,
            serverUrl = baseUrl.toString(),
            targetDeviceId = profile.targetDeviceId,
            targetIdentity = profile.targetIdentity,
            accessKey = profile.token,
        )
        val api = ApiClient(connection.apiBase.toHttpUrl(), connection.credential)
        val info: AppInfo = api.get("/api/v1/app/info")
        if (!info.hasLibrary) throw ConnectionException("远端服务没有打开的素材库")
        waitUntilReady(api)
        return Outcome(
            api = api,
            info = info,
            label = profile.displayName,
            targetDeviceName = null,
            targetLibraryName = profile.targetLibraryId.ifEmpty { null },
        )
    }

    /** 二维码一次下发所有候选地址；逐个验证，哪个连通用哪个。 */
    private suspend fun launchLanCandidates(profile: ConnectionProfile): Outcome {
        var lastError: Exception? = null
        for (baseUrl in profile.candidateBaseUrls()) {
            try {
                return launchLan(profile, baseUrl)
            } catch (error: CancellationException) {
                throw error
            } catch (error: Exception) {
                lastError = error
            }
        }
        throw lastError ?: ConnectionException("没有可用的局域网地址")
    }

    private suspend fun launchLan(
        profile: ConnectionProfile,
        baseUrl: okhttp3.HttpUrl,
    ): Outcome {
        val token = profile.token.trim()
        if (token.isEmpty()) throw ConnectionException("请填写局域网访问 token")
        val api = ApiClient(baseUrl, token)
        val info: AppInfo = api.get("/api/v1/app/info")
        if (!info.hasLibrary) {
            throw ConnectionException("该服务没有打开的素材库，请在桌面端切换素材库后重试")
        }
        waitUntilReady(api)
        return Outcome(
            api = api,
            info = info,
            label = profile.displayName,
            targetDeviceName = null,
            targetLibraryName = null,
        )
    }

    /** 初始索引构建中 daemon 只放行 app/startup；400ms 轮询到 ready。 */
    private suspend fun waitUntilReady(api: ApiClient) {
        val deadline = System.currentTimeMillis() + 30_000
        while (System.currentTimeMillis() < deadline) {
            try {
                val startup: StartupInfo = api.get("/api/v1/app/startup")
                when (startup.status) {
                    "ready" -> return
                    "error" -> throw ConnectionException(
                        startup.message ?: "素材库索引构建失败"
                    )
                }
            } catch (error: ApiException) {
                if (error.isUnauthorized) throw error
                // NOT_READY、网络抖动与服务端 5xx 都继续轮询，直到超时
            }
            delay(400)
        }
        throw ConnectionException("等待素材库就绪超时，请检查桌面端状态")
    }
}
