package com.ryouji.hawk.data.connection

import java.util.UUID
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import okhttp3.HttpUrl
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull

@Serializable
enum class ConnectionKind(val title: String, val defaultScheme: String) {
    @SerialName("lan") Lan("局域网", "http"),
}

class ConnectionException(message: String) : Exception(message)

/** 一条局域网直连配置：daemon 的 peer 端口地址 + 访问密钥。 */
@Serializable
data class ConnectionProfile(
    val id: String = UUID.randomUUID().toString(),
    val kind: ConnectionKind = ConnectionKind.Lan,
    val name: String = "",
    val url: String,
    /** 同一目标的候选地址；二维码可一次下发多个局域网地址。 */
    val urls: List<String> = emptyList(),
    /** 目标端「局域网网站访问」里创建的访问密钥，明文只在创建时显示。 */
    val accessKey: String = "",
) {
    val displayName: String
        get() {
            val trimmed = name.trim()
            return trimmed.ifEmpty {
                runCatching { baseUrl().host }.getOrElse { url }
            }
        }

    fun baseUrl(): HttpUrl = normalizeBaseUrl(url)

    /** 所有候选地址：二维码下发的 urls 优先，缺省回退到主地址。 */
    fun candidateBaseUrls(): List<HttpUrl> {
        val candidates = (listOf(url) + urls)
            .map { it.trim() }
            .filter { it.isNotEmpty() }
            .distinctBy { it.lowercase() }
        if (candidates.isEmpty()) throw ConnectionException("请填写服务地址")
        return candidates.map { normalizeBaseUrl(it) }
    }

    /** 规范化用户输入的地址：补 scheme、去掉路径尾部斜杠与查询参数。 */
    private fun normalizeBaseUrl(rawValue: String): HttpUrl {
        var raw = rawValue.trim()
        if (raw.isEmpty()) throw ConnectionException("请填写服务地址")
        if (!raw.contains("://")) raw = "${kind.defaultScheme}://$raw"
        val parsed = raw.toHttpUrlOrNull()
            ?: throw ConnectionException("服务地址无效，请填写形如 192.168.1.5:27375 的地址")
        val builder = parsed.newBuilder().query(null).fragment(null)
        if (!parsed.encodedPath.endsWith("/")) builder.addPathSegment("")
        return builder.build()
    }
}

@Serializable
data class ConnectionBookmark(
    val profiles: List<ConnectionProfile> = emptyList(),
    val activeId: String? = null,
)
