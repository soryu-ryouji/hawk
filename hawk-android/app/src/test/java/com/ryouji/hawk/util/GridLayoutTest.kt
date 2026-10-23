package com.ryouji.hawk.util

import com.ryouji.hawk.data.model.SkeletonItem
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class GridLayoutTest {
    private fun item(id: String, width: Int, height: Int) = SkeletonItem(
        id = id,
        path = "$id.jpg",
        width = width,
        height = height,
    )

    @Test
    fun `empty items produce no rows`() {
        assertTrue(GridLayout.rows(emptyList(), width = 1000f, targetHeight = 200f).isEmpty())
    }

    @Test
    fun `last row keeps target height`() {
        val rows = GridLayout.rows(listOf(item("a", 1000, 500)), width = 1000f, targetHeight = 200f)
        assertEquals(1, rows.size)
        val cell = rows.first().cells.single()
        assertEquals(200f, cell.height)
        assertEquals(400f, cell.width)
        assertEquals(200f + GridLayout.META_HEIGHT, rows.first().height)
    }

    @Test
    fun `greedy wrapping keeps rows within width`() {
        val items = (1..6).map { item("i$it", 400, 400) }
        val rows = GridLayout.rows(items, width = 1000f, targetHeight = 200f)
        assertEquals(listOf(4, 2), rows.map { it.cells.size })
        rows.forEach { row ->
            val rowWidth = row.cells.sumOf { it.width.toDouble() } +
                (row.cells.size - 1) * GridLayout.GAP
            assertTrue("row width $rowWidth exceeds container", rowWidth <= 1000.0)
        }
    }

    @Test
    fun `non last rows clamp to 1_75x target`() {
        // 两张窄图装一行后，第三张超宽图触发换行：首行反推高度约 586，需夹紧到 1.75×。
        val items = listOf(
            item("tall", 100, 1000),
            item("normal", 1000, 200),
            item("wide", 3000, 150),
        )
        val rows = GridLayout.rows(items, width = 3000f, targetHeight = 200f)
        assertEquals(2, rows.size)
        assertEquals(2, rows.first().cells.size)
        assertEquals(350f, rows.first().cells.first().height)
    }

    @Test
    fun `index ranges map back to source order`() {
        val items = (1..7).map { item("i$it", 400, 400) }
        val rows = GridLayout.rows(items, width = 900f, targetHeight = 200f)
        var expected = 0
        rows.forEach { row ->
            assertEquals(expected, row.startIndex)
            assertEquals(row.startIndex + row.cells.size, row.endIndex)
            assertEquals(row.cells.map { it.index }, (row.startIndex until row.endIndex).toList())
            expected = row.endIndex
        }
        assertEquals(items.size, expected)
    }
}
