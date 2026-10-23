package com.ryouji.hawk.data.net

import com.ryouji.hawk.data.model.ApiEnvelope
import com.ryouji.hawk.data.model.ApiErrorPayload
import com.ryouji.hawk.data.model.HawkJson
import java.io.IOException
import java.net.ConnectException
import java.net.SocketTimeoutException
import java.net.UnknownHostException
import java.util.concurrent.TimeUnit
import javax.net.ssl.SSLException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.KSerializer
import kotlinx.serialization.builtins.serializer
import kotlinx.serialization.serializer
import okhttp3.HttpUrl
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody

class ApiException(
    val code: String,
    override val message: String,
    val status: Int,
) : Exception(message) {
    val isUnauthorized: Boolean get() = status == 401 || code == "UNAUTHORIZED"
    val isNotReady: Boolean get() = code == "NOT_READY" || status == 503
}

/**
 * hawk REST 客户端：所有响应都是 `{ status, data, error }` 信封。
 * 媒体地址把 token 放查询串（Coil 直接按 URL 取图，不需额外请求头）。
 */
class ApiClient(
    val baseUrl: HttpUrl,
    val token: String,
    private val client: OkHttpClient = defaultClient(),
) {
    private val json = HawkJson
    private val jsonMediaType = "application/json".toMediaType()

    suspend fun <T> get(
        path: String,
        query: List<Pair<String, String>> = emptyList(),
        deserializer: KSerializer<T>,
    ): T = send("GET", path, query, null, deserializer)

    suspend inline fun <reified T> get(
        path: String,
        query: List<Pair<String, String>> = emptyList(),
    ): T = get(path, query, serializer())

    suspend fun <T> post(
        path: String,
        bodyJson: String,
        deserializer: KSerializer<T>,
    ): T = send("POST", path, emptyList(), bodyJson, deserializer)

    suspend inline fun <reified Body, reified T> post(path: String, body: Body): T =
        post(path, HawkJson.encodeToString(serializer<Body>(), body), serializer<T>())

    suspend fun delete(path: String) {
        val data = perform("DELETE", path, emptyList(), null)
        if (data.isBlank()) return
        val envelope = runCatching {
            json.decodeFromString(ApiEnvelope.serializer(String.serializer()), data)
        }.getOrNull() ?: return
        if (envelope.status == "error") throw envelopeError(envelope.error, 200)
    }

    fun thumbnailUrl(id: String): String = mediaUrl("/api/v1/item/thumbnail", id)

    fun fileUrl(id: String): String = mediaUrl("/api/v1/item/file", id)

    /** 下载原图（保存到相册用）。 */
    suspend fun fetchFile(id: String): ByteArray {
        val request = Request.Builder()
            .url(fileUrl(id))
            .header("Accept", "*/*")
            .build()
        return withContext(Dispatchers.IO) {
            try {
                val call = client.newCall(request)
                call.timeout().timeout(120, TimeUnit.SECONDS)
                call.execute().use { response ->
                    if (!response.isSuccessful) {
                        throw ApiException(
                            "HTTP_${response.code}",
                            "下载原图失败（HTTP ${response.code}）",
                            response.code,
                        )
                    }
                    response.body.bytes()
                }
            } catch (error: ApiException) {
                throw error
            } catch (error: IOException) {
                throw ApiException("NETWORK", networkMessage(error), 0)
            }
        }
    }

    private suspend fun <T> send(
        method: String,
        path: String,
        query: List<Pair<String, String>>,
        bodyJson: String?,
        deserializer: KSerializer<T>,
    ): T {
        val data = perform(method, path, query, bodyJson)
        val envelope = try {
            json.decodeFromString(ApiEnvelope.serializer(deserializer), data)
        } catch (error: Exception) {
            throw ApiException("DECODE", "无法解析服务端响应", 0)
        }
        if (envelope.status == "error") throw envelopeError(envelope.error, 200)
        return envelope.data ?: throw ApiException("EMPTY_RESPONSE", "服务端返回为空", 200)
    }

    private suspend fun perform(
        method: String,
        path: String,
        query: List<Pair<String, String>>,
        bodyJson: String?,
    ): String {
        val url = buildUrl(path, query)
        val body = bodyJson?.toRequestBody(jsonMediaType)
        val builder = Request.Builder()
            .url(url)
            .header("Accept", "application/json")
            .method(method, body)
        if (token.isNotEmpty()) {
            builder.header("Authorization", "Bearer $token")
        }
        val request = builder.build()
        return withContext(Dispatchers.IO) {
            try {
                client.newCall(request).execute().use { response ->
                    val text = response.body.string()
                    if (!response.isSuccessful) {
                        val payload = parseError(text)
                        throw ApiException(
                            payload?.code ?: "HTTP_${response.code}",
                            payload?.message ?: "请求失败（HTTP ${response.code}）",
                            response.code,
                        )
                    }
                    text
                }
            } catch (error: ApiException) {
                throw error
            } catch (error: IOException) {
                throw ApiException("NETWORK", networkMessage(error), 0)
            }
        }
    }

    private fun buildUrl(path: String, query: List<Pair<String, String>>): HttpUrl {
        val builder = baseUrl.newBuilder()
        path.trim('/').split('/').filter { it.isNotEmpty() }.forEach(builder::addPathSegment)
        query.forEach { (name, value) -> builder.addQueryParameter(name, value) }
        return builder.build()
    }

    private fun mediaUrl(path: String, id: String): String =
        buildUrl(path, listOf("id" to id, "token" to token)).toString()

    private fun parseError(text: String): ApiErrorPayload? = runCatching {
        json.decodeFromString(ErrorEnvelope.serializer(), text).error
    }.getOrNull()

    private fun envelopeError(error: ApiErrorPayload?, status: Int): ApiException =
        ApiException(
            error?.code ?: "INTERNAL",
            error?.message ?: "请求失败",
            status,
        )

    companion object {
        fun defaultClient(): OkHttpClient =
            OkHttpClient.Builder()
                .connectTimeout(15, TimeUnit.SECONDS)
                .readTimeout(60, TimeUnit.SECONDS)
                .writeTimeout(30, TimeUnit.SECONDS)
                .retryOnConnectionFailure(true)
                .build()

        fun networkMessage(error: IOException): String = when (error) {
            is UnknownHostException ->
                "无法连接素材库服务：请确认地址与端口，以及设备是否在同一网络"
            is SocketTimeoutException -> "连接素材库超时，请检查服务地址与网络"
            is ConnectException -> "无法连接素材库服务（连接被拒绝）"
            is SSLException -> "服务端证书校验失败，请检查 HTTPS 配置"
            else -> "无法连接素材库服务（${error.message ?: "网络错误"}）"
        }
    }
}

@kotlinx.serialization.Serializable
private data class ErrorEnvelope(val error: ApiErrorPayload? = null)
