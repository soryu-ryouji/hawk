package com.ryouji.hawk.data.connection

import com.ryouji.hawk.data.model.HawkJson
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

/**
 * 桌面端生成的配对二维码载荷：
 * 局域网 `{ "v": 1, "kind": "lan", "url": "...", "token": "..." }`
 * 广域网 `{ "v": 1, "kind": "remote", "url": "...", "service_key": "...", "key": "..." }`
 */
@Serializable
data class PairingPayload(
    val v: Int,
    val kind: String,
    val url: String,
    val token: String? = null,
    @SerialName("service_key") val serviceKey: String? = null,
    val key: String? = null,
) {
    fun toProfile(): ConnectionProfile = when (kind) {
        "lan" -> {
            val accessToken = token?.trim().orEmpty()
            if (accessToken.isEmpty()) throw ConnectionException("配对二维码缺少访问 token")
            ConnectionProfile(kind = ConnectionKind.Lan, url = url, token = accessToken)
        }

        "remote" -> {
            val secret = serviceKey?.trim().orEmpty()
            val appKeyValue = key?.trim().orEmpty()
            if (secret.isEmpty() || appKeyValue.isEmpty()) {
                throw ConnectionException("配对二维码缺少访问密钥")
            }
            ConnectionProfile(
                kind = ConnectionKind.Remote,
                url = url,
                serviceKey = secret,
                appKey = appKeyValue,
            )
        }

        else -> throw ConnectionException("二维码类型不支持")
    }

    companion object {
        fun decode(code: String): PairingPayload {
            val payload = try {
                HawkJson.decodeFromString(PairingPayload.serializer(), code)
            } catch (error: Exception) {
                throw ConnectionException("这不是 hawk 的配对二维码")
            }
            if (payload.v != 1) {
                throw ConnectionException("二维码版本不支持，请升级 hawk Android 客户端")
            }
            return payload
        }
    }
}
