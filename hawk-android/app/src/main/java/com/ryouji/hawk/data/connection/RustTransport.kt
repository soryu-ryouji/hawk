package com.ryouji.hawk.data.connection

import com.ryouji.hawk.data.model.HawkJson
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

/** hawk-transport C ABI 的本地代理返回。 */
@Serializable
data class NativeConnection(
    @SerialName("session_id") val sessionId: String,
    @SerialName("api_base") val apiBase: String,
    val credential: String,
    val role: String,
    @SerialName("target_device_id") val targetDeviceId: String,
    val error: String? = null,
)

/** 原生 iroh/AccessKey Loopback 传输。UI 只消费本地 HTTP 代理。 */
object RustTransport {
    init {
        System.loadLibrary("hawk_transport")
    }

    @JvmStatic
    external fun connectAccess(
        configDir: String,
        serverUrl: String,
        targetDeviceId: String,
        targetIdentity: String,
        accessKey: String,
    ): String

    @JvmStatic
    external fun disconnect(sessionId: String): String

    @JvmStatic
    external fun freeString(pointer: Long): Unit

    fun connect(
        configDir: String,
        serverUrl: String,
        targetDeviceId: String,
        targetIdentity: String,
        accessKey: String,
    ): NativeConnection {
        val raw = connectAccess(configDir, serverUrl, targetDeviceId, targetIdentity, accessKey)
        val result = HawkJson.decodeFromString(NativeConnection.serializer(), raw)
        if (!result.error.isNullOrEmpty()) throw ConnectionException(result.error)
        return result
    }
}
