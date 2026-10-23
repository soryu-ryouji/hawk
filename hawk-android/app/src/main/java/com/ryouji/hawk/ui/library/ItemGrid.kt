package com.ryouji.hawk.ui.library

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.BrokenImage
import androidx.compose.material.icons.outlined.DeleteOutline
import androidx.compose.material.icons.outlined.PhotoLibrary
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import coil3.compose.AsyncImage
import coil3.request.ImageRequest
import coil3.request.crossfade
import com.ryouji.hawk.data.library.LibraryStore
import com.ryouji.hawk.data.library.LibraryUiState
import com.ryouji.hawk.data.library.displayTitle
import com.ryouji.hawk.data.model.Item
import com.ryouji.hawk.data.model.LibraryView
import com.ryouji.hawk.ui.connect.ErrorBanner
import com.ryouji.hawk.ui.theme.HawkColors
import com.ryouji.hawk.util.Format
import com.ryouji.hawk.util.GridLayout
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ItemGrid(
    store: LibraryStore,
    state: LibraryUiState,
    targetHeight: Float,
    onOpen: (Int) -> Unit,
    modifier: Modifier = Modifier,
) {
    val scope = rememberCoroutineScope()
    var refreshing by remember { mutableStateOf(false) }

    BoxWithConstraints(modifier = modifier.fillMaxSize().background(HawkColors.Bg0)) {
        val contentWidth = (maxWidth - 24.dp).value
        val rows = remember(state.layoutVersion, contentWidth, targetHeight) {
            GridLayout.rows(state.displayItems, contentWidth, targetHeight)
        }

        PullToRefreshBox(
            isRefreshing = refreshing,
            onRefresh = {
                scope.launch {
                    refreshing = true
                    try {
                        store.refreshAll()
                    } finally {
                        refreshing = false
                    }
                }
            },
            modifier = Modifier.fillMaxSize(),
        ) {
            LazyColumn(
                modifier = Modifier.fillMaxSize(),
                contentPadding = PaddingValues(12.dp),
                verticalArrangement = Arrangement.spacedBy(GridLayout.GAP.dp),
            ) {
                items(rows, key = { it.id }) { row ->
                    Row(horizontalArrangement = Arrangement.spacedBy(GridLayout.GAP.dp)) {
                        row.cells.forEach { cell ->
                            ItemCard(
                                cell = cell,
                                detail = state.details[cell.item.key],
                                thumbnailUrl = store.api.thumbnailUrl(cell.item.id),
                                onClick = { onOpen(cell.index) },
                            )
                        }
                    }
                    LaunchedEffect(state.skeletonVersion, row.startIndex) {
                        store.ensureWindow(row.startIndex, row.endIndex)
                    }
                }
            }
        }

        if (state.displayItems.isEmpty()) {
            when {
                state.loading || state.firstPageLoading -> LoadingGrid()
                state.error == null -> EmptyState(store = store, state = state)
            }
        }

        if (state.loading && state.displayItems.isNotEmpty()) {
            Box(
                modifier = Modifier
                    .align(Alignment.TopCenter)
                    .padding(top = 8.dp)
                    .background(HawkColors.Bg1.copy(alpha = 0.9f), RoundedCornerShape(50))
                    .padding(8.dp),
            ) {
                CircularProgressIndicator(
                    modifier = Modifier.size(18.dp),
                    color = HawkColors.Fg1,
                    strokeWidth = 2.dp,
                )
            }
        }

        state.error?.let { message ->
            Box(modifier = Modifier.align(Alignment.TopCenter).padding(12.dp)) {
                ErrorBanner(message = message)
            }
        }
    }
}

@Composable
private fun ItemCard(
    cell: GridLayout.Cell,
    detail: Item?,
    thumbnailUrl: String?,
    onClick: () -> Unit,
) {
    val title = displayTitle(cell.item, detail)
    Column(
        modifier = Modifier
            .width(cell.width.dp)
            .height((cell.height + GridLayout.META_HEIGHT).dp)
            .clip(RoundedCornerShape(4.dp))
            .background(HawkColors.Bg2)
            .clickable(onClick = onClick),
    ) {
        Box(modifier = Modifier.width(cell.width.dp).height(cell.height.dp)) {
            ThumbnailImage(
                url = thumbnailUrl,
                modifier = Modifier.fillMaxSize(),
                contentScale = ContentScale.Crop,
            )
            if (cell.item.star > 0) {
                Text(
                    text = "★${cell.item.star}",
                    color = HawkColors.Star,
                    fontSize = 11.sp,
                    fontWeight = FontWeight.SemiBold,
                    modifier = Modifier
                        .align(Alignment.TopEnd)
                        .padding(4.dp),
                )
            }
        }
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .height(GridLayout.META_HEIGHT.dp)
                .padding(horizontal = 6.dp, vertical = 5.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Text(
                text = title,
                color = HawkColors.Fg0,
                fontSize = 12.sp,
                textAlign = TextAlign.Center,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                text = Format.dimensions(cell.item.width, cell.item.height),
                color = HawkColors.Fg1,
                fontSize = 11.sp,
                maxLines = 1,
            )
        }
    }
}

/**
 * 远程图片：新入库素材的缩略图由后台异步生成，首次 404 时短暂重试，
 * 避免网格留下永久占位（与 iOS 一致）。
 */
@Composable
fun ThumbnailImage(
    url: String?,
    modifier: Modifier = Modifier,
    contentScale: ContentScale = ContentScale.Crop,
    maxPixelSize: Int? = null,
    backgroundColor: Color = HawkColors.Bg2,
) {
    val context = LocalContext.current
    var attempt by remember(url) { mutableIntStateOf(0) }
    var failed by remember(url) { mutableStateOf(false) }

    LaunchedEffect(failed, attempt) {
        if (failed && attempt < 3) {
            delay(1_500)
            failed = false
            attempt += 1
        }
    }

    val requestUrl = remember(url, attempt) {
        when {
            url == null -> null
            attempt == 0 -> url
            else -> "$url&retry=$attempt"
        }
    }
    val request = remember(requestUrl, maxPixelSize) {
        ImageRequest.Builder(context)
            .data(requestUrl)
            .crossfade(true)
            .apply { if (maxPixelSize != null) size(maxPixelSize, maxPixelSize) }
            .build()
    }

    Box(modifier = modifier.background(backgroundColor)) {
        AsyncImage(
            model = request,
            contentDescription = null,
            contentScale = contentScale,
            modifier = Modifier.fillMaxSize(),
            onError = { failed = true },
            onSuccess = { failed = false },
        )
        if (failed) {
            Icon(
                Icons.Outlined.BrokenImage,
                contentDescription = null,
                tint = HawkColors.Fg1.copy(alpha = 0.5f),
                modifier = Modifier.align(Alignment.Center).size(18.dp),
            )
        }
    }
}

@Composable
private fun LoadingGrid() {
    Column(modifier = Modifier.fillMaxSize().background(HawkColors.Bg0)) {
        LazyVerticalGrid(
            columns = GridCells.Adaptive(minSize = 144.dp),
            contentPadding = PaddingValues(12.dp),
            horizontalArrangement = Arrangement.spacedBy(10.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
            modifier = Modifier.weight(1f, fill = false),
        ) {
            items(12) { index ->
                Box(
                    modifier = Modifier
                        .fillMaxWidth()
                        .height(listOf(190.dp, 150.dp, 230.dp)[index % 3])
                        .clip(RoundedCornerShape(4.dp))
                        .background(HawkColors.Bg2),
                )
            }
        }
        Row(
            modifier = Modifier.fillMaxWidth().padding(bottom = 24.dp),
            horizontalArrangement = Arrangement.Center,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            CircularProgressIndicator(
                modifier = Modifier.size(16.dp),
                color = HawkColors.Fg1,
                strokeWidth = 2.dp,
            )
            Spacer(modifier = Modifier.width(8.dp))
            Text(text = "正在加载素材…", color = HawkColors.Fg1, fontSize = 12.sp)
        }
    }
}

@Composable
private fun EmptyState(store: LibraryStore, state: LibraryUiState) {
    val filtered = state.query.hasActiveFilters || state.query.keywords.isNotEmpty()
    Column(
        modifier = Modifier.fillMaxSize().padding(32.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.Center,
    ) {
        Icon(
            imageVector = if (state.view == LibraryView.Trash) {
                Icons.Outlined.DeleteOutline
            } else {
                Icons.Outlined.PhotoLibrary
            },
            contentDescription = null,
            tint = HawkColors.Fg1,
            modifier = Modifier.size(44.dp),
        )
        Text(
            text = if (state.view == LibraryView.Trash) "回收站为空" else "暂无素材",
            color = HawkColors.Fg0,
            fontSize = 16.sp,
            fontWeight = FontWeight.Medium,
            modifier = Modifier.padding(top = 12.dp),
        )
        Text(
            text = when {
                state.query.hasActiveFilters -> "当前筛选条件下没有素材，试试清除筛选。"
                state.query.keywords.isNotEmpty() ->
                    "没有匹配「${state.query.keywords.joinToString(" ")}」的素材"
                else -> "在桌面端导入素材后，这里会自动出现。"
            },
            color = HawkColors.Fg1,
            fontSize = 12.sp,
            textAlign = TextAlign.Center,
            modifier = Modifier.padding(top = 6.dp),
        )
        if (filtered) {
            TextButton(onClick = { store.clearFiltersAndSearch() }) {
                Text("清除筛选与搜索", color = HawkColors.Accent)
            }
        }
    }
}

/** 纯色占位（详情加载中/预览兜底）。 */
@Composable
fun DarkPlaceholder(modifier: Modifier = Modifier, color: Color = HawkColors.Bg2) {
    Box(modifier = modifier.background(color))
}
