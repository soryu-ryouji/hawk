package com.ryouji.hawk.ui.library

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.DeleteOutline
import androidx.compose.material.icons.outlined.Folder
import androidx.compose.material.icons.outlined.GridView
import androidx.compose.material.icons.outlined.Home
import androidx.compose.material.icons.outlined.Info
import androidx.compose.material.icons.outlined.Inbox
import androidx.compose.material.icons.outlined.KeyboardArrowDown
import androidx.compose.material.icons.automirrored.outlined.Label
import androidx.compose.material.icons.automirrored.outlined.LabelOff
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.ryouji.hawk.LibrarySession
import com.ryouji.hawk.data.library.LibraryStore
import com.ryouji.hawk.data.library.LibraryUiState
import com.ryouji.hawk.data.model.FolderNode
import com.ryouji.hawk.data.model.LibraryView
import com.ryouji.hawk.ui.HawkMark
import com.ryouji.hawk.ui.connect.SectionHint
import com.ryouji.hawk.ui.theme.HawkColors

@Composable
fun SidebarContent(
    store: LibraryStore,
    session: LibrarySession,
    onNavigate: () -> Unit,
    onOpenSettings: (() -> Unit)? = null,
    modifier: Modifier = Modifier,
) {
    val state by store.state.collectAsStateWithLifecycle()
    var foldersCollapsed by remember { mutableStateOf(false) }
    var categoriesCollapsed by remember { mutableStateOf(false) }
    var tagsCollapsed by remember { mutableStateOf(false) }
    var expandedFolders by remember { mutableStateOf(setOf<String>()) }

    val folderRows = remember(state.folders, expandedFolders) {
        val out = ArrayList<FolderRowData>()
        state.folders?.let { flattenFolders(it, 0, expandedFolders, out) }
        out
    }

    LazyColumn(modifier = modifier.fillMaxWidth().background(HawkColors.Bg1)) {
        item(key = "header") {
            SidebarHeader(session = session, state = state)
        }

        item(key = "smart") { SmartEntries(state = state, store = store, onNavigate = onNavigate) }

        item(key = "folders-header") {
            SectionHeader(
                title = "文件夹",
                collapsed = foldersCollapsed,
                onToggle = { foldersCollapsed = !foldersCollapsed },
            )
        }
        if (!foldersCollapsed) {
            when {
                state.taxonomyLoading -> item(key = "folders-loading") { SectionHint("加载中…") }
                folderRows.isEmpty() -> item(key = "folders-empty") { SectionHint("暂无文件夹") }
                else -> items(folderRows, key = { "folder-${it.node.path}" }) { row ->
                    FolderRow(
                        data = row,
                        activePath = (state.view as? LibraryView.Folder)?.path,
                        expanded = row.node.path in expandedFolders,
                        onToggleExpand = {
                            expandedFolders = if (row.node.path in expandedFolders) {
                                expandedFolders - row.node.path
                            } else {
                                expandedFolders + row.node.path
                            }
                        },
                        onSelect = {
                            store.select(LibraryView.Folder(row.node.path))
                            onNavigate()
                        },
                    )
                }
            }
        }

        item(key = "categories-header") {
            SectionHeader(
                title = "分类",
                collapsed = categoriesCollapsed,
                onToggle = { categoriesCollapsed = !categoriesCollapsed },
            )
        }
        if (!categoriesCollapsed) {
            when {
                state.taxonomyLoading -> item(key = "categories-loading") { SectionHint("加载中…") }
                state.categories.isEmpty() -> item(key = "categories-empty") { SectionHint("暂无分类") }
                else -> items(state.categories, key = { "category-${it.name}" }) { category ->
                    SidebarRow(
                        title = category.name,
                        count = category.count,
                        active = state.view == LibraryView.Category(category.name),
                        icon = Icons.Outlined.GridView,
                        onClick = {
                            store.select(LibraryView.Category(category.name))
                            onNavigate()
                        },
                    )
                }
            }
        }

        item(key = "tags-header") {
            SectionHeader(
                title = "标签",
                collapsed = tagsCollapsed,
                onToggle = { tagsCollapsed = !tagsCollapsed },
            )
        }
        if (!tagsCollapsed) {
            when {
                state.taxonomyLoading -> item(key = "tags-loading") { SectionHint("加载中…") }
                state.tags.isEmpty() -> item(key = "tags-empty") { SectionHint("暂无标签") }
                else -> items(state.tags, key = { "tag-${it.name}" }) { tag ->
                    SidebarRow(
                        title = tag.name,
                        count = tag.count,
                        active = state.view == LibraryView.Tag(tag.name),
                        icon = Icons.AutoMirrored.Outlined.Label,
                        onClick = {
                            store.select(LibraryView.Tag(tag.name))
                            onNavigate()
                        },
                    )
                }
            }
        }

        if (onOpenSettings != null) {
            item(key = "settings") {
                HorizontalDivider(color = HawkColors.Border)
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .clickable(onClick = onOpenSettings)
                        .padding(horizontal = 12.dp, vertical = 12.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    Icon(
                        imageVector = Icons.Outlined.Info,
                        contentDescription = null,
                        tint = HawkColors.Fg1,
                        modifier = Modifier.size(16.dp),
                    )
                    Text(
                        text = "连接信息",
                        color = HawkColors.Fg0,
                        fontSize = 13.sp,
                    )
                }
            }
        }

        item(key = "bottom") { Spacer(modifier = Modifier.height(20.dp)) }
    }
}

private data class FolderRowData(val node: FolderNode, val depth: Int)

private fun flattenFolders(
    node: FolderNode,
    depth: Int,
    expanded: Set<String>,
    out: MutableList<FolderRowData>,
) {
    node.children.forEach { child ->
        out.add(FolderRowData(child, depth))
        if (child.path in expanded) flattenFolders(child, depth + 1, expanded, out)
    }
}

@Composable
private fun SidebarHeader(session: LibrarySession, state: LibraryUiState) {
    Row(
        modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(9.dp),
    ) {
        HawkMark(size = 28)
        Column(verticalArrangement = Arrangement.spacedBy(1.dp)) {
            Text(
                text = state.library?.name ?: "hawk",
                color = HawkColors.Fg0,
                fontSize = 14.sp,
                fontWeight = FontWeight.SemiBold,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                text = "${if (session.isRemote) "远程" else "局域网"} · ${session.accessLabel}",
                color = HawkColors.Fg1,
                fontSize = 10.sp,
                maxLines = 1,
            )
        }
    }
}

@Composable
private fun SmartEntries(state: LibraryUiState, store: LibraryStore, onNavigate: () -> Unit) {
    fun count(value: Int): Int? = if (state.countsLoading) null else value

    Column(modifier = Modifier.fillMaxWidth().padding(bottom = 6.dp)) {
        SidebarRow(
            title = "全部素材",
            count = count(state.counts.all),
            active = state.view == LibraryView.All,
            icon = Icons.Outlined.GridView,
        ) {
            store.select(LibraryView.All)
            onNavigate()
        }
        SidebarRow(
            title = "根目录素材",
            count = count(state.counts.root),
            active = state.view == LibraryView.Root,
            icon = Icons.Outlined.Home,
        ) {
            store.select(LibraryView.Root)
            onNavigate()
        }
        SidebarRow(
            title = "未分类素材",
            count = count(state.counts.uncategorized),
            active = state.view == LibraryView.Uncategorized,
            icon = Icons.Outlined.Inbox,
        ) {
            store.select(LibraryView.Uncategorized)
            onNavigate()
        }
        SidebarRow(
            title = "未标签素材",
            count = count(state.counts.untagged),
            active = state.view == LibraryView.Untagged,
            icon = Icons.AutoMirrored.Outlined.LabelOff,
        ) {
            store.select(LibraryView.Untagged)
            onNavigate()
        }
        SidebarRow(
            title = "回收站",
            count = count(state.counts.trash),
            active = state.view == LibraryView.Trash,
            icon = Icons.Outlined.DeleteOutline,
        ) {
            store.select(LibraryView.Trash)
            onNavigate()
        }
    }
}

@Composable
private fun SectionHeader(title: String, collapsed: Boolean, onToggle: () -> Unit) {
    val rotation by animateFloatAsState(
        targetValue = if (collapsed) -90f else 0f,
        label = "section-chevron",
    )
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clickable(onClick = onToggle)
            .padding(horizontal = 12.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        Icon(
            Icons.Outlined.KeyboardArrowDown,
            contentDescription = null,
            tint = HawkColors.Fg1,
            modifier = Modifier.size(16.dp).rotate(rotation),
        )
        Text(
            text = title,
            color = HawkColors.Fg1,
            fontSize = 11.sp,
            fontWeight = FontWeight.SemiBold,
        )
    }
}

@Composable
private fun SidebarRow(
    title: String,
    count: Int? = null,
    active: Boolean,
    icon: ImageVector? = null,
    leadingIndent: Int = 12,
    onClick: () -> Unit,
) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .background(if (active) HawkColors.Bg3 else HawkColors.Bg1)
            .clickable(onClick = onClick)
            .padding(start = leadingIndent.dp, end = 12.dp, top = 7.dp, bottom = 7.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(7.dp),
    ) {
        if (icon != null) {
            Icon(
                imageVector = icon,
                contentDescription = null,
                tint = if (active) HawkColors.Fg0 else HawkColors.Fg1,
                modifier = Modifier.size(16.dp),
            )
        }
        Text(
            text = title,
            color = if (active) Color.White else HawkColors.Fg0,
            fontSize = 13.sp,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f),
        )
        if (count != null) {
            Text(
                text = count.toString(),
                color = if (active) Color.White else HawkColors.Fg1,
                fontSize = 11.sp,
                fontWeight = if (active) FontWeight.SemiBold else FontWeight.Normal,
            )
        }
    }
}

@Composable
private fun FolderRow(
    data: FolderRowData,
    activePath: String?,
    expanded: Boolean,
    onToggleExpand: () -> Unit,
    onSelect: () -> Unit,
) {
    val rotation by animateFloatAsState(
        targetValue = if (expanded) 0f else -90f,
        label = "folder-chevron",
    )
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .background(if (activePath == data.node.path) HawkColors.Bg3 else HawkColors.Bg1)
            .clickable(onClick = onSelect)
            .padding(start = (data.depth * 14 + 4).dp, end = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(
            modifier = Modifier
                .size(24.dp)
                .clickable(enabled = data.node.children.isNotEmpty(), onClick = onToggleExpand),
            contentAlignment = Alignment.Center,
        ) {
            if (data.node.children.isNotEmpty()) {
                Icon(
                    Icons.Outlined.KeyboardArrowDown,
                    contentDescription = null,
                    tint = HawkColors.Fg1,
                    modifier = Modifier.size(14.dp).rotate(rotation),
                )
            }
        }
        Icon(
            Icons.Outlined.Folder,
            contentDescription = null,
            tint = if (activePath == data.node.path) HawkColors.Fg0 else HawkColors.Fg1,
            modifier = Modifier.size(16.dp),
        )
        Spacer(modifier = Modifier.width(7.dp))
        Text(
            text = data.node.name,
            color = if (activePath == data.node.path) Color.White else HawkColors.Fg0,
            fontSize = 13.sp,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f).padding(vertical = 7.dp),
        )
        Text(
            text = data.node.count.toString(),
            color = HawkColors.Fg1,
            fontSize = 11.sp,
            modifier = Modifier.padding(vertical = 7.dp),
        )
    }
}
