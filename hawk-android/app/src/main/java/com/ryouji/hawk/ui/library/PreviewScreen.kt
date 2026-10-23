package com.ryouji.hawk.ui.library

import android.Manifest
import android.content.pm.PackageManager
import android.widget.Toast
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.detectVerticalDragGestures
import androidx.compose.foundation.gestures.rememberTransformableState
import androidx.compose.foundation.gestures.transformable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.rememberPagerState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Close
import androidx.compose.material.icons.outlined.Download
import androidx.compose.material.icons.outlined.Info
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.content.ContextCompat
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.ryouji.hawk.R
import com.ryouji.hawk.data.library.LibraryStore
import com.ryouji.hawk.data.library.displayTitle
import com.ryouji.hawk.data.model.Item
import com.ryouji.hawk.data.model.ItemKey
import com.ryouji.hawk.data.model.SkeletonItem
import com.ryouji.hawk.ui.theme.HawkColors
import com.ryouji.hawk.ui.theme.fromHex
import com.ryouji.hawk.util.Format
import com.ryouji.hawk.util.PhotoSaver
import kotlin.math.abs
import kotlin.math.max
import kotlinx.coroutines.launch

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun PreviewScreen(store: LibraryStore, startIndex: Int, onClose: () -> Unit) {
    val state by store.state.collectAsStateWithLifecycle()
    val items = state.displayItems
    if (items.isEmpty()) {
        LaunchedEffect(Unit) { onClose() }
        return
    }

    val initialPage = startIndex.coerceIn(0, items.lastIndex)
    val pagerState = rememberPagerState(initialPage = initialPage) { items.size }
    val context = LocalContext.current
    val savePermissionDenied = stringResource(R.string.save_permission_denied)
    val scope = rememberCoroutineScope()

    var detail by remember { mutableStateOf<Item?>(null) }
    var showInfo by remember { mutableStateOf(false) }
    var saving by remember { mutableStateOf(false) }
    var liveScale by remember { mutableFloatStateOf(1f) }

    val currentPage = pagerState.currentPage
    val currentItem = items.getOrNull(currentPage)
    val currentDetail = detail?.takeIf { it.id == currentItem?.id }

    LaunchedEffect(currentPage) {
        detail = null
        detail = store.detail(currentPage)
        liveScale = 1f
    }

    val saveCurrent: () -> Unit = {
        val item = currentItem
        if (item != null && !saving) {
            saving = true
            scope.launch {
                try {
                    val name = currentDetail?.displayName ?: fallbackName(item)
                    PhotoSaver.save(
                        context = context,
                        api = store.api,
                        id = item.id,
                        fileName = name,
                        ext = ItemKey.ext(item.path),
                    )
                    Toast.makeText(context, "已保存到相册", Toast.LENGTH_SHORT).show()
                } catch (error: Exception) {
                    Toast.makeText(context, error.message ?: "保存失败", Toast.LENGTH_LONG).show()
                } finally {
                    saving = false
                }
            }
        }
    }

    val permissionLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission()
    ) { granted ->
        if (granted) {
            saveCurrent()
        } else {
            Toast.makeText(context, savePermissionDenied, Toast.LENGTH_LONG).show()
        }
    }
    val onSave: () -> Unit = {
        val needsPermission = PhotoSaver.needsLegacyPermission() &&
            ContextCompat.checkSelfPermission(
                context,
                Manifest.permission.WRITE_EXTERNAL_STORAGE,
            ) != PackageManager.PERMISSION_GRANTED
        if (needsPermission) {
            permissionLauncher.launch(Manifest.permission.WRITE_EXTERNAL_STORAGE)
        } else {
            saveCurrent()
        }
    }

    Box(modifier = Modifier.fillMaxSize().background(Color.Black)) {
        HorizontalPager(
            state = pagerState,
            modifier = Modifier.fillMaxSize(),
            userScrollEnabled = liveScale <= 1.01f,
            beyondViewportPageCount = 1,
        ) { page ->
            val item = items[page]
            PreviewPage(
                item = item,
                fileUrl = store.api.fileUrl(item.id),
                isCurrent = page == currentPage,
                onScaleChanged = { liveScale = it },
                onClose = onClose,
            )
        }

        Row(
            modifier = Modifier
                .align(Alignment.TopStart)
                .fillMaxWidth()
                .background(
                    Brush.verticalGradient(
                        0.0f to Color.Black.copy(alpha = 0.78f),
                        0.55f to Color.Black.copy(alpha = 0.62f),
                        1.0f to Color.Transparent,
                    )
                )
                .statusBarsPadding()
                .padding(horizontal = 14.dp, vertical = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            RoundIconButton(onClick = onClose, description = "关闭预览") {
                Icon(
                    Icons.Outlined.Close,
                    contentDescription = null,
                    tint = Color.White,
                    modifier = Modifier.size(18.dp),
                )
            }
            Column(
                modifier = Modifier.weight(1f).padding(horizontal = 12.dp),
                verticalArrangement = Arrangement.spacedBy(2.dp),
            ) {
                Text(
                    text = currentItem?.let { displayTitle(it, currentDetail) }.orEmpty(),
                    color = Color.White,
                    fontSize = 13.sp,
                    fontWeight = FontWeight.Medium,
                    maxLines = 1,
                )
                Text(
                    text = subtitle(currentPage, items.size, currentItem, liveScale),
                    color = HawkColors.Fg1,
                    fontSize = 10.sp,
                    maxLines = 1,
                )
            }
            RoundIconButton(onClick = onSave, description = "保存到相册", enabled = !saving) {
                if (saving) {
                    CircularProgressIndicator(
                        modifier = Modifier.size(16.dp),
                        color = Color.White,
                        strokeWidth = 2.dp,
                    )
                } else {
                    Icon(
                        Icons.Outlined.Download,
                        contentDescription = null,
                        tint = Color.White,
                        modifier = Modifier.size(18.dp),
                    )
                }
            }
            Spacer(modifier = Modifier.width(10.dp))
            RoundIconButton(onClick = { showInfo = true }, description = "素材信息") {
                Icon(
                    Icons.Outlined.Info,
                    contentDescription = null,
                    tint = Color.White,
                    modifier = Modifier.size(18.dp),
                )
            }
        }
    }

    if (showInfo) {
        ModalBottomSheet(
            onDismissRequest = { showInfo = false },
            sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
            containerColor = HawkColors.Bg1,
        ) {
            PreviewInfoSheet(detail = currentDetail)
        }
    }
}

@Composable
private fun RoundIconButton(
    onClick: () -> Unit,
    description: String,
    enabled: Boolean = true,
    content: @Composable () -> Unit,
) {
    Box(
        modifier = Modifier
            .size(34.dp)
            .clip(CircleShape)
            .background(Color.Black.copy(alpha = 0.35f))
            .semantics { contentDescription = description }
    ) {
        IconButton(onClick = onClick, enabled = enabled, modifier = Modifier.fillMaxSize()) {
            content()
        }
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun PreviewPage(
    item: SkeletonItem,
    fileUrl: String,
    isCurrent: Boolean,
    onScaleChanged: (Float) -> Unit,
    onClose: () -> Unit,
) {
    var scale by remember(item.key) { mutableFloatStateOf(1f) }
    var offset by remember(item.key) { mutableStateOf(Offset.Zero) }
    var dragY by remember(item.key) { mutableFloatStateOf(0f) }

    LaunchedEffect(scale, isCurrent) {
        if (isCurrent) onScaleChanged(scale)
    }

    BoxWithConstraints(modifier = Modifier.fillMaxSize()) {
        val container = Size(constraints.maxWidth.toFloat(), constraints.maxHeight.toFloat())
        val display = displaySize(container, item.aspectRatio)
        val transformState = rememberTransformableState { _, zoomChange, panChange, _ ->
            val nextScale = (scale * zoomChange).coerceIn(1f, 5f)
            scale = nextScale
            offset = if (nextScale <= 1.01f) {
                Offset.Zero
            } else {
                clampOffset(offset + panChange, nextScale, container, display)
            }
        }
        val dim = if (scale <= 1.01f && dragY != 0f) {
            (1f - (abs(dragY) / (container.height * 0.5f)).coerceIn(0f, 0.5f))
        } else {
            1f
        }

        Box(
            modifier = Modifier
                .fillMaxSize()
                .pointerInput(item.key, scale <= 1.01f) {
                    if (scale <= 1.01f) {
                        detectVerticalDragGestures(
                            onDragEnd = {
                                if (dragY > 120.dp.toPx()) {
                                    onClose()
                                } else {
                                    dragY = 0f
                                }
                            },
                            onDragCancel = { dragY = 0f },
                        ) { change, amount ->
                            change.consume()
                            dragY += amount
                        }
                    }
                }
                .pointerInput(item.key) {
                    detectTapGestures(
                        onDoubleTap = {
                            if (scale > 1.01f) {
                                scale = 1f
                                offset = Offset.Zero
                            } else {
                                scale = 2.5f
                                offset = Offset.Zero
                            }
                        }
                    )
                }
                .transformable(state = transformState, canPan = { scale > 1.01f }),
            contentAlignment = Alignment.Center,
        ) {
            Box(
                modifier = Modifier
                    .fillMaxSize()
                    .graphicsLayer {
                        scaleX = scale
                        scaleY = scale
                        translationX = offset.x
                        translationY = offset.y + dragY
                    }
                    .alpha(dim),
            ) {
                ThumbnailImage(
                    url = fileUrl,
                    modifier = Modifier.fillMaxSize(),
                    contentScale = ContentScale.Fit,
                    maxPixelSize = 4096,
                    backgroundColor = Color.Black,
                )
            }
        }
    }
}

private fun displaySize(container: Size, aspect: Float): Size {
    if (container.width <= 0f || container.height <= 0f || aspect <= 0f) return container
    val containerAspect = container.width / container.height
    return if (aspect > containerAspect) {
        Size(container.width, container.width / aspect)
    } else {
        Size(container.height * aspect, container.height)
    }
}

private fun clampOffset(offset: Offset, scale: Float, container: Size, display: Size): Offset {
    val maxX = max(0f, (display.width * scale - container.width) / 2f)
    val maxY = max(0f, (display.height * scale - container.height) / 2f)
    return Offset(
        x = offset.x.coerceIn(-maxX, maxX),
        y = offset.y.coerceIn(-maxY, maxY),
    )
}

private fun fallbackName(item: SkeletonItem): String {
    val name = ItemKey.baseName(item.path)
    val ext = ItemKey.ext(item.path)
    return if (ext.isEmpty()) name else "$name.$ext"
}

private fun subtitle(index: Int, total: Int, item: SkeletonItem?, scale: Float): String {
    val parts = mutableListOf<String>()
    if (total > 0) parts.add("${index + 1} / $total")
    if (item != null) {
        parts.add(Format.dimensions(item.width, item.height))
        parts.add(Format.bytes(item.size))
    }
    if (scale > 1.01f) parts.add(String.format(java.util.Locale.US, "%.1f×", scale))
    return parts.joinToString(" · ")
}

@OptIn(ExperimentalMaterial3Api::class, ExperimentalLayoutApi::class)
@Composable
private fun PreviewInfoSheet(detail: Item?) {
    val uriHandler = LocalUriHandler.current
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .verticalScroll(rememberScrollState())
            .padding(horizontal = 20.dp)
            .padding(bottom = 28.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        Text(
            text = "素材信息",
            color = HawkColors.Fg0,
            fontSize = 16.sp,
            fontWeight = FontWeight.SemiBold,
        )
        if (detail == null) {
            Row(
                modifier = Modifier.fillMaxWidth().padding(vertical = 24.dp),
                horizontalArrangement = Arrangement.Center,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                CircularProgressIndicator(
                    modifier = Modifier.size(18.dp),
                    color = HawkColors.Fg1,
                    strokeWidth = 2.dp,
                )
                Spacer(modifier = Modifier.width(10.dp))
                Text("正在读取素材信息…", color = HawkColors.Fg1, fontSize = 12.sp)
            }
            return@Column
        }

        Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(
                text = detail.displayName,
                color = HawkColors.Fg0,
                fontSize = 16.sp,
                fontWeight = FontWeight.SemiBold,
            )
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                InfoPill(Format.dimensions(detail.width, detail.height))
                InfoPill(Format.bytes(detail.size))
                if (detail.star > 0) {
                    InfoPill("★".repeat(detail.star), color = HawkColors.Star)
                }
            }
            Text(
                text = Format.date(detail.modificationTime),
                color = HawkColors.Fg1,
                fontSize = 11.sp,
            )
        }

        if (detail.tags.isNotEmpty()) {
            ChipSection(title = "标签", values = detail.tags, tint = HawkColors.Accent)
        }
        if (detail.categories.isNotEmpty()) {
            ChipSection(
                title = "分类",
                values = detail.categories,
                tint = Color.fromHex("#8a6bff"),
            )
        }
        if (detail.folders.isNotEmpty()) {
            ChipSection(
                title = "文件夹",
                values = detail.folders.map(ItemKey::displayPath),
                tint = HawkColors.Fg1,
            )
        }
        if (detail.palette.isNotEmpty()) {
            Column(verticalArrangement = Arrangement.spacedBy(7.dp)) {
                SectionTitle("调色板")
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    detail.palette.take(10).forEach { entry ->
                        Box(
                            modifier = Modifier
                                .size(22.dp)
                                .clip(CircleShape)
                                .background(Color.fromHex(entry.color))
                        )
                    }
                }
            }
        }
        detail.annotation?.takeIf { it.isNotBlank() }?.let { annotation ->
            Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                SectionTitle("备注")
                Text(text = annotation, color = HawkColors.Fg0, fontSize = 13.sp)
            }
        }
        detail.url?.takeIf { it.isNotBlank() }?.let { url ->
            Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                SectionTitle("来源")
                Text(
                    text = url,
                    color = HawkColors.Accent,
                    fontSize = 12.sp,
                    maxLines = 1,
                    modifier = Modifier.clip(RoundedCornerShape(4.dp)).padding(vertical = 2.dp),
                )
                androidx.compose.material3.TextButton(onClick = {
                    runCatching { uriHandler.openUri(url) }
                }) {
                    Text("打开链接", color = HawkColors.Accent, fontSize = 12.sp)
                }
            }
        }
        if (detail.paths.isNotEmpty()) {
            Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                SectionTitle("位置")
                detail.paths.forEach { path ->
                    Text(
                        text = ItemKey.displayPath(path),
                        color = HawkColors.Fg1,
                        fontSize = 11.sp,
                        fontFamily = FontFamily.Monospace,
                    )
                }
            }
        }
    }
}

@Composable
private fun SectionTitle(text: String) {
    Text(
        text = text,
        color = HawkColors.Fg1,
        fontSize = 11.sp,
        fontWeight = FontWeight.SemiBold,
    )
}

@Composable
private fun InfoPill(text: String, color: Color = HawkColors.Fg1) {
    Text(
        text = text,
        color = color,
        fontSize = 11.sp,
        modifier = Modifier
            .clip(RoundedCornerShape(50))
            .background(HawkColors.Bg2)
            .padding(horizontal = 8.dp, vertical = 3.dp),
    )
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun ChipSection(title: String, values: List<String>, tint: Color) {
    Column(verticalArrangement = Arrangement.spacedBy(7.dp)) {
        SectionTitle(title)
        FlowRow(
            horizontalArrangement = Arrangement.spacedBy(6.dp),
            verticalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            values.forEach { value ->
                Text(
                    text = value,
                    color = tint,
                    fontSize = 11.sp,
                    maxLines = 1,
                    modifier = Modifier
                        .clip(RoundedCornerShape(50))
                        .background(tint.copy(alpha = 0.12f))
                        .padding(horizontal = 8.dp, vertical = 4.dp),
                )
            }
        }
    }
}
