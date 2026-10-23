package com.ryouji.hawk.util

import android.Manifest
import android.content.ContentValues
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.os.Environment
import android.provider.MediaStore
import androidx.core.content.ContextCompat
import com.ryouji.hawk.data.net.ApiClient

class SavePhotoException(message: String) : Exception(message)

object PhotoSaver {
    /** 保存原图到系统相册（Pictures/hawk）。API 29+ 走 MediaStore，不需要存储权限。 */
    suspend fun save(
        context: Context,
        api: ApiClient,
        id: String,
        fileName: String,
        ext: String,
    ) {
        if (Build.VERSION.SDK_INT <= Build.VERSION_CODES.P &&
            ContextCompat.checkSelfPermission(context, Manifest.permission.WRITE_EXTERNAL_STORAGE) !=
            PackageManager.PERMISSION_GRANTED
        ) {
            throw SavePhotoException("没有相册写入权限，请在系统设置中允许 hawk 添加照片")
        }
        val bytes = api.fetchFile(id)
        val mime = mimeType(ext)
        val values = ContentValues().apply {
            put(MediaStore.Images.Media.DISPLAY_NAME, fileName)
            put(MediaStore.Images.Media.MIME_TYPE, mime)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                put(
                    MediaStore.Images.Media.RELATIVE_PATH,
                    "${Environment.DIRECTORY_PICTURES}/hawk",
                )
                put(MediaStore.Images.Media.IS_PENDING, 1)
            }
        }
        val resolver = context.contentResolver
        val uri = resolver.insert(MediaStore.Images.Media.EXTERNAL_CONTENT_URI, values)
            ?: throw SavePhotoException("无法创建相册条目")
        try {
            resolver.openOutputStream(uri)?.use { it.write(bytes) }
                ?: throw SavePhotoException("无法写入相册")
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                values.clear()
                values.put(MediaStore.Images.Media.IS_PENDING, 0)
                resolver.update(uri, values, null, null)
            }
        } catch (error: Exception) {
            resolver.delete(uri, null, null)
            throw if (error is SavePhotoException) error
            else SavePhotoException("保存到相册失败（${error.message ?: "未知错误"}）")
        }
    }

    fun needsLegacyPermission(): Boolean =
        Build.VERSION.SDK_INT <= Build.VERSION_CODES.P

    private fun mimeType(ext: String): String = when (ext.lowercase()) {
        "png" -> "image/png"
        "webp" -> "image/webp"
        "gif" -> "image/gif"
        "bmp" -> "image/bmp"
        "avif" -> "image/avif"
        "heic" -> "image/heic"
        "jpeg", "jpg", "" -> "image/jpeg"
        else -> "image/${ext.lowercase()}"
    }
}
