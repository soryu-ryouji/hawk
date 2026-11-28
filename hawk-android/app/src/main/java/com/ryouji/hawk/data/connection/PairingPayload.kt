package com.ryouji.hawk.data.connection

import com.ryouji.hawk.data.model.HawkJson
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

/** V3 配对载荷：局域网候选地址 + 访问密钥。字段都可选，但 accessKey 与至少一个 endpoint 必填。 */
@Serializable
data class PairingPayload(
    val v: Int,
    @SerialName("target_library_id") val targetLibraryId: String? = null,
    @SerialName("access_key") val accessKey: String = "",
    @SerialName("lan_endpoints") val lanEndpoints: List<String> = emptyList(),
) {
    fun toProfile(): ConnectionProfile {
        val key = accessKey.trim()
        if (key.isEmpty()) throw ConnectionException("配对二维码缺少访问密钥")
        val candidates = lanEndpoints.map { it.trim() }.filter { it.isNotEmpty() }
        if (candidates.isEmpty()) throw ConnectionException("配对二维码缺少局域网地址")
        return ConnectionProfile(
            kind = ConnectionKind.Lan,
            url = candidates.first(),
            urls = candidates,
            accessKey = key,
        )
    }

    companion object {
        const val VERSION = 3

        fun decode(code: String): PairingPayload {
            val payload = try {
                HawkJson.decodeFromString(PairingPayload.serializer(), code)
            } catch (error: Exception) {
                throw ConnectionException("这不是 hawk 的配对二维码")
            }
            if (payload.v != VERSION) {
                throw ConnectionException("二维码版本不支持，请升级 hawk Android 客户端")
            }
            return payload
        }
    }
}
