package com.ryouji.hawk.data.connection

import com.ryouji.hawk.data.model.AppInfo
import com.ryouji.hawk.data.model.StartupInfo
import com.ryouji.hawk.data.net.ApiClient
import com.ryouji.hawk.data.net.ApiException
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.delay
import okhttp3.HttpUrl

/** 连接启动：逐个尝试候选局域网地址，用访问密钥校验 daemon 并等素材库就绪。 */
object ConnectionLauncher {
    data class Outcome(
        val api: ApiClient,
        val info: AppInfo,
        val label: String,
    )

    suspend fun launch(profile: ConnectionProfile): Outcome {
        var lastError: Exception? = null
        for (baseUrl in profile.candidateBaseUrls()) {
            try {
                return launchCandidate(profile, baseUrl)
            } catch (error: CancellationException) {
                throw error
            } catch (error: Exception) {
                lastError = error
            }
        }
        throw lastError ?: ConnectionException("没有可用的局域网地址")
    }

    private suspend fun launchCandidate(
        profile: ConnectionProfile,
        baseUrl: HttpUrl,
    ): Outcome {
        val accessKey = profile.accessKey.trim()
        if (accessKey.isEmpty()) throw ConnectionException("请填写访问密钥")
        val api = ApiClient(baseUrl, accessKey)
        val info: AppInfo = api.get("/api/v1/app/info")
        if (!info.hasLibrary) {
            throw ConnectionException("该服务没有打开的素材库，请在桌面端切换素材库后重试")
        }
        waitUntilReady(api)
        return Outcome(api = api, info = info, label = profile.displayName)
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
