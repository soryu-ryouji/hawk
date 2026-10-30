package com.ryouji.hawk.ui.library

import android.content.res.Configuration
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
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
import androidx.compose.material.icons.outlined.Folder
import androidx.compose.material.icons.outlined.Info
import androidx.compose.material.icons.outlined.Menu
import androidx.compose.material.icons.outlined.MoreVert
import androidx.compose.material.icons.outlined.PhotoLibrary
import androidx.compose.material.icons.outlined.Public
import androidx.compose.material.icons.outlined.Search
import androidx.compose.material3.DrawerValue
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.IconButton
import androidx.compose.material3.ModalDrawerSheet
import androidx.compose.material3.ModalNavigationDrawer
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
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
import androidx.compose.ui.platform.LocalConfiguration
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

private enum class PhoneTab { Library, Browse }

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun LibraryScreen(viewModel: AppViewModel, session: LibrarySession) {
    val store = session.store
    val state by store.state.collectAsStateWithLifecycle()
    val thumbSize by viewModel.thumbSize.collectAsStateWithLifecycle()
    val configuration = LocalConfiguration.current
    val isTablet = configuration.smallestScreenWidthDp >= 600
    val isLandscape = configuration.orientation == Configuration.ORIENTATION_LANDSCAPE
    val permanentSidebar = isTablet && isLandscape
    val drawerState = rememberDrawerState(DrawerValue.Closed)
    val scope = rememberCoroutineScope()
    var phoneTab by remember { mutableStateOf(PhoneTab.Library) }

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

    Box(modifier = Modifier.fillMaxSize().background(HawkColors.Bg0)) {
        when {
            permanentSidebar -> {
                Row(modifier = Modifier.fillMaxSize()) {
                    SidebarContent(
                        store = store,
                        session = session,
                        onNavigate = {},
                        onOpenSettings = { showSettings = true },
                        modifier = Modifier
                            .width(300.dp)
                            .fillMaxHeight()
                            .statusBarsPadding(),
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
                        onSwitchLibrary = viewModel::showConnectScreen,
                        onDisconnect = viewModel::disconnect,
                        modifier = Modifier.weight(1f),
                    )
                }
            }

            isTablet -> {
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
                                onOpenSettings = { showSettings = true },
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
                        onSwitchLibrary = viewModel::showConnectScreen,
                        onDisconnect = viewModel::disconnect,
                    )
                }
            }

            else -> {
                Scaffold(
                    containerColor = HawkColors.Bg0,
                    bottomBar = {
                        LibraryBottomBar(
                            selected = phoneTab,
                            onSelect = { phoneTab = it },
                        )
                    },
                ) { padding ->
                    Box(modifier = Modifier.fillMaxSize().padding(padding)) {
                        when (phoneTab) {
                            PhoneTab.Library -> LibraryPane(
                                session = session,
                                state = state,
                                thumbSize = thumbSize,
                                showSidebarButton = false,
                                onOpenSidebar = {},
                                onOpenPreview = { previewIndex = it },
                                onOpenFilter = { showFilter = true },
                                onOpenSettings = { showSettings = true },
                                onSwitchLibrary = viewModel::showConnectScreen,
                                onDisconnect = viewModel::disconnect,
                            )

                            PhoneTab.Browse -> BrowsePane(
                                session = session,
                                onOpenSettings = { showSettings = true },
                                onNavigate = { phoneTab = PhoneTab.Library },
                            )
                        }
                    }
                }
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

@Composable
private fun LibraryBottomBar(selected: PhoneTab, onSelect: (PhoneTab) -> Unit) {
    NavigationBar(containerColor = HawkColors.Bg1, tonalElevation = 0.dp) {
        NavigationBarItem(
            selected = selected == PhoneTab.Library,
            onClick = { onSelect(PhoneTab.Library) },
            icon = { Icon(Icons.Outlined.PhotoLibrary, contentDescription = null) },
            label = { Text("素材") },
        )
        NavigationBarItem(
            selected = selected == PhoneTab.Browse,
            onClick = { onSelect(PhoneTab.Browse) },
            icon = { Icon(Icons.Outlined.Folder, contentDescription = null) },
            label = { Text("浏览") },
        )
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun BrowsePane(
    session: LibrarySession,
    onOpenSettings: () -> Unit,
    onNavigate: () -> Unit,
) {
    val state by session.store.state.collectAsStateWithLifecycle()
    Scaffold(
        containerColor = HawkColors.Bg0,
        topBar = {
            TopAppBar(
                title = {
                    Text(
                        text = "素材库",
                        color = HawkColors.Fg0,
                        fontSize = 16.sp,
                        fontWeight = FontWeight.Medium,
                    )
                },
                actions = {
                    IconButton(onClick = onOpenSettings) {
                        Icon(
                            Icons.Outlined.Info,
                            contentDescription = "连接信息",
                            tint = HawkColors.Fg0,
                        )
                    }
                },
                colors = TopAppBarDefaults.topAppBarColors(containerColor = HawkColors.Bg0),
            )
        },
    ) { padding ->
        SidebarContent(
            store = session.store,
            session = session,
            onNavigate = onNavigate,
            onOpenSettings = onOpenSettings,
            modifier = Modifier.fillMaxSize().padding(padding),
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
    onSwitchLibrary: () -> Unit,
    onDisconnect: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val store = session.store
    val focusManager = LocalFocusManager.current
    val scope = rememberCoroutineScope()
    var filterSortOpen by remember { mutableStateOf(false) }
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
                    Box {
                        IconButton(onClick = { filterSortOpen = true }) {
                            Icon(
                                imageVector = Icons.Outlined.FilterList,
                                contentDescription = "排序与筛选",
                                tint = if (state.query.hasActiveFilters) {
                                    HawkColors.Accent
                                } else {
                                    HawkColors.Fg0
                                },
                            )
                        }
                        FilterSortMenu(
                            expanded = filterSortOpen,
                            state = state,
                            onDismiss = { filterSortOpen = false },
                            onSelectSort = {
                                store.setSort(it)
                                filterSortOpen = false
                            },
                            onOpenFilter = {
                                filterSortOpen = false
                                onOpenFilter()
                            },
                            onClearFilters = {
                                store.clearFilters()
                                filterSortOpen = false
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
                            onSwitchLibrary = {
                                moreMenuOpen = false
                                onSwitchLibrary()
                            },
                            onDisconnect = {
                                moreMenuOpen = false
                                onDisconnect()
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
private fun FilterSortMenu(
    expanded: Boolean,
    state: LibraryUiState,
    onDismiss: () -> Unit,
    onSelectSort: (SortChoice) -> Unit,
    onOpenFilter: () -> Unit,
    onClearFilters: () -> Unit,
) {
    DropdownMenu(
        expanded = expanded,
        onDismissRequest = onDismiss,
        containerColor = HawkColors.Bg2,
    ) {
        Text(
            text = "排序",
            color = HawkColors.Fg1,
            fontSize = 11.sp,
            fontWeight = FontWeight.SemiBold,
            modifier = Modifier.padding(horizontal = 12.dp, vertical = 6.dp),
        )
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
                onClick = { onSelectSort(choice) },
            )
        }
        HorizontalDivider(color = HawkColors.Border)
        DropdownMenuItem(
            text = { Text("筛选…", fontSize = 13.sp) },
            leadingIcon = {
                Icon(
                    Icons.Outlined.FilterList,
                    contentDescription = null,
                    tint = HawkColors.Fg1,
                    modifier = Modifier.size(16.dp),
                )
            },
            onClick = onOpenFilter,
        )
        if (state.query.hasActiveFilters) {
            DropdownMenuItem(
                text = { Text("清除筛选", color = HawkColors.Danger, fontSize = 13.sp) },
                onClick = onClearFilters,
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
    onSwitchLibrary: () -> Unit,
    onDisconnect: () -> Unit,
) {
    DropdownMenu(
        expanded = expanded,
        onDismissRequest = onDismiss,
        containerColor = HawkColors.Bg2,
    ) {
        DropdownMenuItem(
            text = { Text("刷新素材库", fontSize = 13.sp) },
            onClick = onRefresh,
        )
        if (state.view != LibraryView.All) {
            DropdownMenuItem(
                text = { Text("返回上层", fontSize = 13.sp) },
                onClick = onNavigateUp,
            )
        }
        HorizontalDivider(color = HawkColors.Border)
        DropdownMenuItem(
            text = { Text("连接信息", fontSize = 13.sp) },
            onClick = onSettings,
        )
        DropdownMenuItem(
            text = { Text("切换素材库", fontSize = 13.sp) },
            onClick = onSwitchLibrary,
        )
        DropdownMenuItem(
            text = { Text("断开连接", color = HawkColors.Danger, fontSize = 13.sp) },
            onClick = onDisconnect,
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
