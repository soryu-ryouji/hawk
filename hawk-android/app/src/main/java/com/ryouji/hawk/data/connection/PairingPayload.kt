package com.ryouji.hawk.data.connection

import com.ryouji.hawk.data.model.HawkJson
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

/** V2 统一配对：AccessKey + target_identity + LAN/WAN endpoints，不携带 tenant key。 */
@Serializable
data class PairingPayload(
    val v: Int,
    @SerialName("target_library_id") val targetLibraryId: String? = null,
    @SerialName("target_device_id") val targetDeviceId: String? = null,
    @SerialName("target_identity") val targetIdentity: String = "",
    @SerialName("tls_fingerprint") val tlsFingerprint: String? = null,
    @SerialName("access_key") val accessKey: String = "",
    @SerialName("lan_endpoints") val lanEndpoints: List<String> = emptyList(),
    val wan: WanEndpoint? = null,
) {
    @Serializable
    data class WanEndpoint(@SerialName("server_url") val serverUrl: String)

    fun toProfile(): ConnectionProfile {
        if (targetIdentity.isBlank() || accessKey.isBlank()) {
            throw ConnectionException("配对二维码缺少目标身份或 AccessKey")
        }
        val candidates = lanEndpoints.map { it.trim() }.filter { it.isNotEmpty() }
        val serverUrl = wan?.serverUrl?.trim().orEmpty()
        val kind = if (serverUrl.isNotEmpty()) ConnectionKind.Remote else ConnectionKind.Lan
        val url = if (serverUrl.isNotEmpty()) serverUrl else candidates.firstOrNull().orEmpty()
        if (url.isEmpty()) throw ConnectionException("配对二维码缺少访问地址")
        return ConnectionProfile(
            kind = kind,
            url = url,
            urls = candidates,
            token = accessKey,
            appKey = accessKey,
            targetIdentity = targetIdentity,
            targetLibraryId = targetLibraryId.orEmpty(),
            targetDeviceId = targetDeviceId.orEmpty(),
            tlsFingerprint = tlsFingerprint.orEmpty(),
        )
    }

    companion object {
        fun decode(code: String): PairingPayload {
            val payload = try {
                HawkJson.decodeFromString(PairingPayload.serializer(), code)
            } catch (error: Exception) {
                throw ConnectionException("这不是 hawk 的配对二维码")
            }
            if (payload.v != 2) {
                throw ConnectionException("二维码版本不支持，请升级 hawk Android 客户端")
            }
            return payload
        }
    }
}
