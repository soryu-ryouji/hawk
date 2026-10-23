package com.ryouji.hawk.util

import com.ryouji.hawk.data.model.SkeletonItem
import kotlin.math.floor
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt

/**
 * 齐行网格布局，算法与 hawk-app/web/src/domains/library/logic/layout.ts 一致：
 * 贪心装行，非末行按容器宽反推行高并夹紧到目标高度的 0.5×–1.75×；末行保持目标高度。
 * 尺寸单位为 dp（Compose 侧直接 dp → px，避免与屏幕密度耦合）。
 */
object GridLayout {
    const val GAP = 10f
    const val META_HEIGHT = 48f

    data class Cell(
        val index: Int,
        val item: SkeletonItem,
        val width: Float,
        val height: Float,
    ) {
        val id: String get() = item.key
    }

    data class Row(
        val cells: List<Cell>,
        val startIndex: Int,
        val endIndex: Int,
        val height: Float,
    ) {
        val id: String get() = cells.first().item.key
    }

    fun rows(items: List<SkeletonItem>, width: Float, targetHeight: Float): List<Row> {
        if (width <= 0f || items.isEmpty() || targetHeight <= 0f) return emptyList()
        val rows = ArrayList<Row>()
        val pending = ArrayList<Pair<Int, SkeletonItem>>()
        var ratiosSum = 0f

        fun flush(isLast: Boolean) {
            if (pending.isEmpty()) return
            val totalGap = (pending.size - 1) * GAP
            val fitHeight = (width - totalGap) / ratiosSum
            val ideal = if (isLast) {
                targetHeight
            } else {
                min(max(fitHeight, targetHeight * 0.5f), targetHeight * 1.75f)
            }
            val height = max(1f, floor(min(ideal, fitHeight)))
            val cells = pending.map { (index, item) ->
                Cell(
                    index = index,
                    item = item,
                    width = max(1f, (height * item.aspectRatio).roundToInt().toFloat()),
                    height = height,
                )
            }
            rows.add(
                Row(
                    cells = cells,
                    startIndex = pending.first().first,
                    endIndex = pending.last().first + 1,
                    height = height + META_HEIGHT,
                )
            )
            pending.clear()
            ratiosSum = 0f
        }

        items.forEachIndexed { index, item ->
            val ratio = item.aspectRatio
            if (pending.isNotEmpty() &&
                (ratiosSum + ratio) * targetHeight + pending.size * GAP > width
            ) {
                flush(isLast = false)
            }
            pending.add(index to item)
            ratiosSum += ratio
        }
        flush(isLast = true)
        return rows
    }
}
