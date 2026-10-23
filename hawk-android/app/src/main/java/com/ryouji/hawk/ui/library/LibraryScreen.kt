package com.ryouji.hawk.ui.library

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Check
import androidx.compose.material.icons.outlined.Clear
import androidx.compose.material.icons.outlined.FilterList
import androidx.compose.material.icons.outlined.Menu
import androidx.compose.material.icons.outlined.MoreVert
import androidx.compose.material.icons.outlined.Public
import androidx.compose.material.icons.outlined.Search
import androidx.compose.material.icons.outlined.SwapVert
import androidx.compose.material3.DrawerValue
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ModalDrawerSheet
import androidx.compose.material3.ModalNavigationDrawer
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextField
import androidx.compose.material3.TextFieldDefaults
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.material3.VerticalDivider
import androidx.compose.material3.rememberDrawerState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusManager
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.LifecycleResumeEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.ryouji.hawk.AppViewModel
import com.ryouji.hawk.LibrarySession
import com.ryouji.hawk.data.library.LibraryStore
import com.ryouji.hawk.data.library.LibraryUiState
import com.ryouji.hawk.data.model.LibraryView
import com.ryouji.hawk.data.model.SortChoice
import com.ryouji.hawk.ui.theme.HawkColors
import com.ryouji.hawk.ui.theme.fromHex
import kotlinx.coroutines.launch

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun LibraryScreen(viewModel: AppViewModel, session: LibrarySession) {
    val store = session.store
    val state by store.state.collectAsStateWithLifecycle()
    val thumbSize by viewModel.thumbSize.collectAsStateWithLifecycle()
    val drawerState = rememberDrawerState(DrawerValue.Closed)
    val scope = rememberCoroutineScope()

    var previewIndex by remember { mutableStateOf<Int?>(null) }
    var showFilter by remember { mutableStateOf(false) }
    var showSettings by remember { mutableStateOf(false) }

    previewIndex?.let { index ->
        BackHandler(enabled = true) { previewIndex = null }
        PreviewScreen(
            store = store,
            startIndex = index,
            onClose = { previewIndex = null },
        )
        return
    }

    LifecycleResumeEffect(Unit) {
        scope.launch { store.refreshIfStale() }
        onPauseOrDispose {}
    }

    BoxWithConstraints(modifier = Modifier.fillMaxSize().background(HawkColors.Bg0)) {
        val wide = maxWidth >= 840.dp
        if (wide) {
            Row(modifier = Modifier.fillMaxSize()) {
                SidebarContent(
                    store = store,
                    session = session,
                    onNavigate = {},
                    modifier = Modifier.width(280.dp).fillMaxHeight(),
                )
                VerticalDivider(color = HawkColors.Border)
                LibraryPane(
                    session = session,
                    state = state,
                    thumbSize = thumbSize,
                    showSidebarButton = false,
                    onOpenSidebar = {},
                    onOpenPreview = { previewIndex = it },
                    onOpenFilter = { showFilter = true },
                    onOpenSettings = { showSettings = true },
                    onDisconnect = viewModel::disconnect,
                    modifier = Modifier.weight(1f),
                )
            }
        } else {
            ModalNavigationDrawer(
                drawerState = drawerState,
                drawerContent = {
                    ModalDrawerSheet(
                        drawerState = drawerState,
                        drawerContainerColor = HawkColors.Bg1,
                        modifier = Modifier.width(300.dp),
                    ) {
                        SidebarContent(
                            store = store,
                            session = session,
                            onNavigate = { scope.launch { drawerState.close() } },
                            modifier = Modifier.fillMaxHeight(),
                        )
                    }
                },
            ) {
                LibraryPane(
                    session = session,
                    state = state,
                    thumbSize = thumbSize,
                    showSidebarButton = true,
                    onOpenSidebar = { scope.launch { drawerState.open() } },
                    onOpenPreview = { previewIndex = it },
                    onOpenFilter = { showFilter = true },
                    onOpenSettings = { showSettings = true },
                    onDisconnect = viewModel::disconnect,
                )
            }
        }
    }

    if (showFilter) {
        FilterSheet(store = store, onDismiss = { showFilter = false })
    }
    if (showSettings) {
        SettingsSheet(
            viewModel = viewModel,
            session = session,
            onDismiss = { showSettings = false },
        )
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun LibraryPane(
    session: LibrarySession,
    state: LibraryUiState,
    thumbSize: Float,
    showSidebarButton: Boolean,
    onOpenSidebar: () -> Unit,
    onOpenPreview: (Int) -> Unit,
    onOpenFilter: () -> Unit,
    onOpenSettings: () -> Unit,
    onDisconnect: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val store = session.store
    val focusManager = LocalFocusManager.current
    val scope = rememberCoroutineScope()
    var sortMenuOpen by remember { mutableStateOf(false) }
    var moreMenuOpen by remember { mutableStateOf(false) }

    Scaffold(
        modifier = modifier,
        containerColor = HawkColors.Bg0,
        topBar = {
            TopAppBar(
                title = {
                    Text(
                        text = state.viewTitle,
                        color = HawkColors.Fg0,
                        fontSize = 16.sp,
                        fontWeight = FontWeight.Medium,
                    )
                },
                navigationIcon = {
                    if (showSidebarButton) {
                        IconButton(onClick = onOpenSidebar) {
                            Icon(
                                Icons.Outlined.Menu,
                                contentDescription = "打开导航",
                                tint = HawkColors.Fg0,
                            )
                        }
                    }
                },
                actions = {
                    IconButton(onClick = onOpenFilter) {
                        Icon(
                            imageVector = Icons.Outlined.FilterList,
                            contentDescription = "筛选",
                            tint = if (state.query.hasActiveFilters) {
                                HawkColors.Accent
                            } else {
                                HawkColors.Fg0
                            },
                        )
                    }
                    Box {
                        IconButton(onClick = { sortMenuOpen = true }) {
                            Icon(
                                Icons.Outlined.SwapVert,
                                contentDescription = "排序",
                                tint = HawkColors.Fg0,
                            )
                        }
                        SortMenu(
                            expanded = sortMenuOpen,
                            state = state,
                            onDismiss = { sortMenuOpen = false },
                            onSelect = {
                                store.setSort(it)
                                sortMenuOpen = false
                            },
                        )
                    }
                    Box {
                        IconButton(onClick = { moreMenuOpen = true }) {
                            Icon(
                                Icons.Outlined.MoreVert,
                                contentDescription = "更多",
                                tint = HawkColors.Fg0,
                            )
                        }
                        MoreMenu(
                            expanded = moreMenuOpen,
                            state = state,
                            onDismiss = { moreMenuOpen = false },
                            onNavigateUp = {
                                moreMenuOpen = false
                                store.navigateUp()
                            },
                            onRefresh = {
                                moreMenuOpen = false
                                scope.launch { store.refreshAll() }
                            },
                            onSettings = {
                                moreMenuOpen = false
                                onOpenSettings()
                            },
                        )
                    }
                },
                colors = TopAppBarDefaults.topAppBarColors(containerColor = HawkColors.Bg0),
            )
        },
    ) { padding ->
        Column(modifier = Modifier.fillMaxSize().padding(padding)) {
            if (session.isRemote) {
                RemoteBanner(label = session.label, onDisconnect = onDisconnect)
            }
            SearchRow(
                value = state.searchText,
                onValueChange = store::setSearchText,
                onSubmit = {
                    store.submitSearch()
                    focusManager.clearFocus()
                },
                focusManager = focusManager,
            )
            if (state.query.hasActiveFilters) {
                ActiveFilterBar(store = store, state = state)
            }
            ItemGrid(
                store = store,
                state = state,
                targetHeight = thumbSize,
                onOpen = onOpenPreview,
                modifier = Modifier.weight(1f),
            )
        }
    }
}

@Composable
private fun SearchRow(
    value: String,
    onValueChange: (String) -> Unit,
    onSubmit: () -> Unit,
    focusManager: FocusManager,
) {
    TextField(
        value = value,
        onValueChange = onValueChange,
        placeholder = { Text("搜索名称与备注", color = HawkColors.Fg1, fontSize = 13.sp) },
        leadingIcon = {
            Icon(
                Icons.Outlined.Search,
                contentDescription = null,
                tint = HawkColors.Fg1,
                modifier = Modifier.size(18.dp),
            )
        },
        trailingIcon = {
            if (value.isNotEmpty()) {
                IconButton(onClick = {
                    onValueChange("")
                    onSubmit()
                }) {
                    Icon(
                        Icons.Outlined.Clear,
                        contentDescription = "清除搜索",
                        tint = HawkColors.Fg1,
                        modifier = Modifier.size(16.dp),
                    )
                }
            }
        },
        singleLine = true,
        shape = RoundedCornerShape(8.dp),
        keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
        keyboardActions = KeyboardActions(onSearch = { onSubmit() }),
        colors = TextFieldDefaults.colors(
            focusedContainerColor = HawkColors.Bg2,
            unfocusedContainerColor = HawkColors.Bg2,
            focusedIndicatorColor = Color.Transparent,
            unfocusedIndicatorColor = Color.Transparent,
            focusedTextColor = HawkColors.Fg0,
            unfocusedTextColor = HawkColors.Fg0,
            cursorColor = HawkColors.Accent,
        ),
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 12.dp, vertical = 6.dp)
            .height(48.dp),
    )
}

@Composable
private fun ActiveFilterBar(store: LibraryStore, state: LibraryUiState) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .background(HawkColors.Bg1)
            .horizontalScroll(rememberScrollState())
            .padding(horizontal = 12.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        state.query.star?.let { star ->
            FilterChip(label = "$star 星", tint = HawkColors.Star) { store.setStar(null) }
        }
        state.query.size?.let { size ->
            FilterChip(label = size.label, tint = HawkColors.Accent) { store.setSize(null) }
        }
        state.query.color?.let { color ->
            FilterChip(
                label = color,
                tint = Color.fromHex(color),
                showDot = true,
            ) { store.setColor(null) }
        }
        Text(
            text = "清除全部",
            color = HawkColors.Fg1,
            fontSize = 11.sp,
            modifier = Modifier
                .clip(RoundedCornerShape(50))
                .clickable { store.clearFilters() }
                .padding(horizontal = 8.dp, vertical = 4.dp),
        )
    }
}

@Composable
private fun FilterChip(
    label: String,
    tint: Color,
    showDot: Boolean = false,
    onClear: () -> Unit,
) {
    Row(
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(5.dp),
        modifier = Modifier
            .clip(RoundedCornerShape(50))
            .background(HawkColors.Bg3)
            .padding(horizontal = 9.dp, vertical = 4.dp),
    ) {
        if (showDot) {
            Box(
                modifier = Modifier
                    .size(10.dp)
                    .clip(CircleShape)
                    .background(tint)
            )
        } else {
            Box(modifier = Modifier.size(6.dp).clip(CircleShape).background(tint))
        }
        Text(text = label, color = HawkColors.Fg0, fontSize = 11.sp)
        Text(
            text = "×",
            color = HawkColors.Fg1,
            fontSize = 12.sp,
            modifier = Modifier
                .clip(CircleShape)
                .clickable(onClick = onClear)
                .padding(horizontal = 2.dp),
        )
    }
}

@Composable
private fun SortMenu(
    expanded: Boolean,
    state: LibraryUiState,
    onDismiss: () -> Unit,
    onSelect: (SortChoice) -> Unit,
) {
    DropdownMenu(
        expanded = expanded,
        onDismissRequest = onDismiss,
        containerColor = HawkColors.Bg2,
    ) {
        SortChoice.all.forEach { choice ->
            DropdownMenuItem(
                text = { Text(choice.label, fontSize = 13.sp) },
                leadingIcon = {
                    if (state.currentSort == choice) {
                        Icon(
                            Icons.Outlined.Check,
                            contentDescription = null,
                            tint = HawkColors.Accent,
                            modifier = Modifier.size(16.dp),
                        )
                    } else {
                        Spacer(modifier = Modifier.size(16.dp))
                    }
                },
                onClick = { onSelect(choice) },
            )
        }
    }
}

@Composable
private fun MoreMenu(
    expanded: Boolean,
    state: LibraryUiState,
    onDismiss: () -> Unit,
    onNavigateUp: () -> Unit,
    onRefresh: () -> Unit,
    onSettings: () -> Unit,
) {
    DropdownMenu(
        expanded = expanded,
        onDismissRequest = onDismiss,
        containerColor = HawkColors.Bg2,
    ) {
        if (state.view != LibraryView.All) {
            DropdownMenuItem(
                text = { Text("返回上级", fontSize = 13.sp) },
                onClick = onNavigateUp,
            )
        }
        DropdownMenuItem(
            text = { Text("刷新素材库", fontSize = 13.sp) },
            onClick = onRefresh,
        )
        DropdownMenuItem(
            text = { Text("连接信息", fontSize = 13.sp) },
            onClick = onSettings,
        )
    }
}

@Composable
private fun RemoteBanner(label: String, onDisconnect: () -> Unit) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .background(HawkColors.Bg1)
            .padding(horizontal = 12.dp, vertical = 5.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(7.dp),
    ) {
        Icon(
            Icons.Outlined.Public,
            contentDescription = null,
            tint = HawkColors.Fg1,
            modifier = Modifier.size(12.dp),
        )
        Text(
            text = "当前浏览：$label",
            color = HawkColors.Fg1,
            fontSize = 11.sp,
            maxLines = 1,
            modifier = Modifier.weight(1f),
        )
        Text(
            text = "×",
            color = HawkColors.Fg1,
            fontSize = 15.sp,
            modifier = Modifier
                .clip(CircleShape)
                .clickable(onClick = onDisconnect)
                .padding(horizontal = 6.dp, vertical = 2.dp),
        )
    }
}
