package com.ryouji.hawk.data.model

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json

/**
 * API 契约：字段与 hawk-daemon 的 server-rest-api-v1.md 一一对应。
 * 请求/响应用 @SerialName 显式对齐 snake_case，不做全局命名策略。
 */
val HawkJson = Json {
    ignoreUnknownKeys = true
    explicitNulls = false
    encodeDefaults = false
    isLenient = true
}

// MARK: - 服务端信息

@Serializable
data class AppInfo(
    val version: String,
    val platform: String? = null,
    val role: String,
    val channel: String,
    @SerialName("has_library") val hasLibrary: Boolean,
    val lan: LanInfo? = null,
)

@Serializable
data class LanInfo(
    val active: Boolean = false,
    val port: Int? = null,
    val error: String? = null,
)

@Serializable
data class StartupInfo(
    val status: String,
    val phase: String? = null,
    val processed: Int? = null,
    val total: Int? = null,
    val message: String? = null,
)

@Serializable
data class LibraryInfo(
    val name: String,
    val path: String? = null,
    @SerialName("application_version") val applicationVersion: String? = null,
    @SerialName("storage_mode") val storageMode: String? = null,
    @SerialName("modification_time") val modificationTime: Long? = null,
)

@Serializable
data class FolderNode(
    val path: String,
    val name: String,
    val count: Int = 0,
    @SerialName("modification_time") val modificationTime: Long? = null,
    val children: List<FolderNode> = emptyList(),
)

@Serializable
data class TaxonInfo(val name: String, val count: Int = 0)

@Serializable
data class GlobalFilterSnapshot(
    val folders: List<String> = emptyList(),
    val categories: List<String> = emptyList(),
    val tags: List<String> = emptyList(),
)

@Serializable
data class SkeletonItem(
    val id: String,
    val path: String,
    val width: Int,
    val height: Int,
    val star: Int = 0,
    val size: Long = 0,
) {
    val key: String get() = "$id\n$path"
    val aspectRatio: Float
        get() = if (width > 0 && height > 0) width.toFloat() / height.toFloat() else 1f
}

@Serializable
data class Item(
    val id: String,
    val path: String = "",
    val name: String,
    val ext: String = "",
    val width: Int = 0,
    val height: Int = 0,
    val size: Long = 0,
    val url: String? = null,
    val tags: List<String> = emptyList(),
    val categories: List<String> = emptyList(),
    val paths: List<String> = emptyList(),
    val folders: List<String> = emptyList(),
    val star: Int = 0,
    val annotation: String? = null,
    @SerialName("modification_time") val modificationTime: Long = 0,
    val palette: List<PaletteColor> = emptyList(),
) {
    val key: String get() = "$id\n${path.ifEmpty { paths.firstOrNull().orEmpty() }}"
    val displayName: String get() = if (ext.isEmpty()) name else "$name.$ext"
}

@Serializable
data class PaletteColor(val color: String, val percentage: Double = 0.0)

@Serializable
data class ItemListResponse(
    val items: List<Item> = emptyList(),
    val total: Int = 0,
    @SerialName("total_size") val totalSize: Long = 0,
    val offset: Int = 0,
    val limit: Int = 0,
)

@Serializable
data class ItemSkeletonResponse(
    val items: List<SkeletonItem> = emptyList(),
    @SerialName("total_size") val totalSize: Long = 0,
)

@Serializable
data class GateResponse(
    @SerialName("gate_token") val gateToken: String,
    @SerialName("expires_in") val expiresIn: Int = 0,
)

@Serializable
data class LoginResponse(
    @SerialName("browser_token") val browserToken: String,
    val role: String,
    @SerialName("device_id") val deviceId: String,
)

@Serializable
data class ViewerSession(
    @SerialName("session_token") val sessionToken: String,
    val role: String,
    @SerialName("target_device_id") val targetDeviceId: String,
    @SerialName("target_name") val targetName: String = "",
    @SerialName("target_library_name") val targetLibraryName: String = "",
)

// MARK: - 请求

@Serializable
data class ItemListRequest(
    val ids: List<String>? = null,
    val keywords: List<String>? = null,
    val tags: List<String>? = null,
    val categories: List<String>? = null,
    val star: Int? = null,
    val folders: List<String>? = null,
    @SerialName("folders_exact") val foldersExact: Boolean? = null,
    @SerialName("without_categories") val withoutCategories: Boolean? = null,
    @SerialName("without_tags") val withoutTags: Boolean? = null,
    @SerialName("exclude_folders") val excludeFolders: List<String>? = null,
    @SerialName("exclude_categories") val excludeCategories: List<String>? = null,
    @SerialName("exclude_tags") val excludeTags: List<String>? = null,
    val color: String? = null,
    @SerialName("min_side_gte") val minSideGte: Int? = null,
    @SerialName("min_side_lte") val minSideLte: Int? = null,
    @SerialName("in_trash") val inTrash: Boolean? = null,
    @SerialName("order_by") val orderBy: String = "modification_time",
    val order: String = "desc",
    val offset: Int? = null,
    val limit: Int? = null,
)

@Serializable
data class GateRequest(@SerialName("tenant_key") val tenantKey: String)

@Serializable
data class LoginRequest(
    @SerialName("gate_token") val gateToken: String,
    val key: String,
)

@Serializable
data class OpenSessionRequest(
    @SerialName("target_device_id") val targetDeviceId: String,
    val role: String = "viewer",
)

// MARK: - 本地浏览状态

sealed interface LibraryView {
    data object All : LibraryView
    data object Root : LibraryView
    data object Uncategorized : LibraryView
    data object Untagged : LibraryView
    data object Trash : LibraryView
    data class Folder(val path: String) : LibraryView
    data class Category(val name: String) : LibraryView
    data class Tag(val name: String) : LibraryView

    val title: String
        get() = when (this) {
            All -> "全部素材"
            Root -> "根目录素材"
            Uncategorized -> "未分类素材"
            Untagged -> "未标签素材"
            Trash -> "回收站"
            is Folder -> path.substringAfterLast('/')
            is Category -> name
            is Tag -> name
        }

    val isGlobal: Boolean
        get() = this is All || this is Root || this is Uncategorized || this is Untagged
}

sealed interface SizeFilter {
    data class Side(val min: Int?, val max: Int?) : SizeFilter

    val label: String
        get() {
            val side = this as Side
            return when {
                side.min != null && side.max != null -> "${side.min}p – ${side.max}p"
                side.min != null -> "≥ ${side.min}p"
                side.max != null -> "≤ ${side.max}p"
                else -> "尺寸"
            }
        }
}

data class QueryState(
    val keywords: List<String> = emptyList(),
    val star: Int? = null,
    val color: String? = null,
    val size: SizeFilter? = null,
    val orderBy: String = "modification_time",
    val order: String = "desc",
) {
    val hasActiveFilters: Boolean get() = star != null || color != null || size != null
}

data class SortChoice(
    val id: String,
    val label: String,
    val orderBy: String,
    val order: String,
) {
    companion object {
        val all = listOf(
            SortChoice("mtime-desc", "修改时间 ↓", "modification_time", "desc"),
            SortChoice("mtime-asc", "修改时间 ↑", "modification_time", "asc"),
            SortChoice("name-asc", "名称 ↑", "name", "asc"),
            SortChoice("name-desc", "名称 ↓", "name", "desc"),
            SortChoice("size-desc", "大小 ↓", "size", "desc"),
            SortChoice("size-asc", "大小 ↑", "size", "asc"),
            SortChoice("star-desc", "评分 ↓", "star", "desc"),
            SortChoice("star-asc", "评分 ↑", "star", "asc"),
        )

        fun current(query: QueryState): SortChoice? =
            all.firstOrNull { it.orderBy == query.orderBy && it.order == query.order }
    }
}

data class LibraryCounts(
    val all: Int = 0,
    val root: Int = 0,
    val uncategorized: Int = 0,
    val untagged: Int = 0,
    val trash: Int = 0,
)

/** 路径工具：回收站路径对用户隐藏 `.hawk/trash/` 前缀。 */
object ItemKey {
    private const val TRASH_PREFIX = ".hawk/trash/"

    fun displayPath(path: String): String =
        if (path.startsWith(TRASH_PREFIX)) path.removePrefix(TRASH_PREFIX) else path

    fun fileName(path: String, fallback: String = ""): String {
        val visible = displayPath(path)
        val name = visible.substringAfterLast('/', "")
        return name.ifEmpty { fallback }
    }

    fun baseName(path: String, fallback: String = ""): String {
        val file = fileName(path, fallback)
        val dot = file.lastIndexOf('.')
        return if (dot > 0) file.substring(0, dot) else file
    }

    fun ext(path: String, fallback: String = ""): String {
        val file = fileName(path, fallback)
        val dot = file.lastIndexOf('.')
        return if (dot >= 0 && dot < file.length - 1) {
            file.substring(dot + 1).lowercase()
        } else {
            fallback.lowercase()
        }
    }
}

// MARK: - 服务端信封

@Serializable
data class ApiErrorPayload(val code: String? = null, val message: String? = null)

@Serializable
data class ApiEnvelope<T>(
    val status: String,
    val data: T? = null,
    val error: ApiErrorPayload? = null,
)
