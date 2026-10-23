package com.ryouji.hawk.data.connection

import java.util.UUID
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import okhttp3.HttpUrl
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull

@Serializable
enum class ConnectionKind(val title: String, val defaultScheme: String) {
    @SerialName("lan") Lan("局域网", "http"),
    @SerialName("remote") Remote("广域网", "https"),
}

class ConnectionException(message: String) : Exception(message)

/** 一条连接配置。局域网用 url + token；广域网用 url + serviceKey + appKey。 */
@Serializable
data class ConnectionProfile(
    val id: String = UUID.randomUUID().toString(),
    val kind: ConnectionKind,
    val name: String = "",
    val url: String,
    val token: String = "",
    val serviceKey: String = "",
    val appKey: String = "",
) {
    val displayName: String
        get() {
            val trimmed = name.trim()
            return trimmed.ifEmpty {
                runCatching { baseUrl().host }.getOrElse { url }
            }
        }

    /** 规范化用户输入的地址：补 scheme、去掉路径尾部斜杠与查询参数。 */
    fun baseUrl(): HttpUrl {
        var raw = url.trim()
        if (raw.isEmpty()) throw ConnectionException("请填写服务地址")
        if (!raw.contains("://")) raw = "${kind.defaultScheme}://$raw"
        val parsed = raw.toHttpUrlOrNull()
            ?: throw ConnectionException(
                "服务地址无效，请填写形如 192.168.1.5:27372 或 https://hawk.example.com 的地址"
            )
        if (parsed.scheme != "http" && parsed.scheme != "https") {
            throw ConnectionException("服务地址只支持 http 或 https")
        }
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
