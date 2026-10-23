package com.ryouji.hawk.util

import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

object Format {
    fun bytes(value: Long): String {
        if (value < 1024) return "$value B"
        val units = arrayOf("KB", "MB", "GB", "TB")
        var size = value.toDouble() / 1024
        var index = 0
        while (size >= 1024 && index < units.size - 1) {
            size /= 1024
            index++
        }
        return if (size >= 100) {
            String.format(Locale.US, "%.0f %s", size, units[index])
        } else {
            String.format(Locale.US, "%.1f %s", size, units[index])
        }
    }

    fun date(milliseconds: Long): String {
        val formatter = SimpleDateFormat("yyyy-MM-dd HH:mm", Locale.getDefault())
        return formatter.format(Date(milliseconds))
    }

    fun dimensions(width: Int, height: Int): String = "$width × $height"
}
