package com.ryouji.hawk.data.connection

import android.content.Context
import com.ryouji.hawk.data.model.HawkJson
import androidx.core.content.edit
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * 连接配置存储：应用私有的 SharedPreferences（不参与备份，避免凭证离开设备）。
 * 与 iOS Keychain 的差别记录在 docs/frontend/android-app.md。
 */
class ConnectionStore(context: Context) {
    private val prefs = context.getSharedPreferences("hawk.connections", Context.MODE_PRIVATE)
    private val _bookmark = MutableStateFlow(load())
    val bookmark: StateFlow<ConnectionBookmark> = _bookmark.asStateFlow()

    val profiles: List<ConnectionProfile> get() = _bookmark.value.profiles
    val activeId: String? get() = _bookmark.value.activeId

    val activeProfile: ConnectionProfile?
        get() = activeId?.let { id -> profiles.firstOrNull { it.id == id } }

    /** 同一类型 + 同一地址视为同一条连接（更新凭证而不是堆叠重复项）。 */
    fun upsert(profile: ConnectionProfile): ConnectionProfile {
        val current = _bookmark.value
        if (current.profiles.any { it.id == profile.id }) {
            val next = current.copy(
                profiles = current.profiles.map { if (it.id == profile.id) profile else it }
            )
            save(next)
            return profile
        }
        val normalized = profile.url.trim()
        val duplicate = current.profiles.firstOrNull {
            it.kind == profile.kind && it.url.equals(normalized, ignoreCase = true)
        }
        if (duplicate != null) {
            val updated = profile.copy(id = duplicate.id)
            save(
                current.copy(
                    profiles = current.profiles.map { if (it.id == duplicate.id) updated else it }
                )
            )
            return updated
        }
        save(current.copy(profiles = current.profiles + profile))
        return profile
    }

    fun setActive(id: String?) {
        save(_bookmark.value.copy(activeId = id))
    }

    fun remove(id: String) {
        val current = _bookmark.value
        save(
            current.copy(
                profiles = current.profiles.filterNot { it.id == id },
                activeId = if (current.activeId == id) null else current.activeId,
            )
        )
    }

    private fun load(): ConnectionBookmark {
        val raw = prefs.getString(KEY_BOOKMARK, null) ?: return ConnectionBookmark()
        return runCatching {
            HawkJson.decodeFromString(ConnectionBookmark.serializer(), raw)
        }.getOrElse { ConnectionBookmark() }
    }

    private fun save(bookmark: ConnectionBookmark) {
        prefs.edit {
            putString(KEY_BOOKMARK, HawkJson.encodeToString(ConnectionBookmark.serializer(), bookmark))
        }
        _bookmark.value = bookmark
    }

    private companion object {
        const val KEY_BOOKMARK = "bookmark.v1"
    }
}
