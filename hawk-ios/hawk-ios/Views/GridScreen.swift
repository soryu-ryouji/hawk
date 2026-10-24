import SwiftUI
import UIKit

struct GridScreen: View {
    @Environment(AppModel.self) private var app
    let context: LibraryContext
    private let isRoot: Bool

    @State private var store: GridStore
    @State private var previewTarget: PreviewTarget?
    @State private var showFilter = false
    @State private var showSettings = false
    @State private var exporter = MediaExporter()
    @AppStorage("hawk.ios.thumbSize") private var thumbSize = 0.0
    @Environment(\.horizontalSizeClass) private var horizontalSizeClass
    @Namespace private var zoomNamespace

    /// 未手工调过（0）时按设备给默认目标行高：手机约三列，iPad 更宽松
    private var targetHeight: CGFloat {
        thumbSize > 0 ? thumbSize : (horizontalSizeClass == .regular ? 240 : 130)
    }

    init(context: LibraryContext, view: LibraryView, isRoot: Bool) {
        self.context = context
        self.isRoot = isRoot
        _store = State(initialValue: GridStore(context: context, view: view))
    }

    var body: some View {
        VStack(spacing: 0) {
            if let label = context.scanningLabel {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text(label)
                        .font(.caption)
                        .foregroundStyle(HawkTheme.fg1)
                    Spacer(minLength: 0)
                }
                .padding(.horizontal, HawkTheme.screenPadding)
                .padding(.vertical, 6)
                .background(HawkTheme.bg1)
            }

            if store.hasActiveFilters {
                ActiveFilterBar(store: store)
            }

            if let error = store.error {
                ErrorBanner(message: error) {
                    store.load()
                }
                .padding(.horizontal, HawkTheme.screenPadding)
                .padding(.top, 8)
            }

            if let target = store.lockedTarget {
                LockScreen(context: context, target: target) {
                    store.load()
                }
            } else {
                ItemGridView(
                    context: context,
                    store: store,
                    targetHeight: targetHeight,
                    zoomNamespace: zoomNamespace,
                    onOpen: { openPreview($0) },
                    onInfo: { openPreview($0, showsInfo: true) },
                    onSave: { item in Task { await exporter.save(item, api: context.api) } },
                    onShare: { item, name in
                        Task { await exporter.share(item, name: name, api: context.api) }
                    },
                    onCopyName: copyName
                )
            }
        }
        .background(HawkTheme.bg0)
        .navigationTitle(store.viewTitle)
        .navigationBarTitleDisplayMode(isRoot ? .large : .inline)
        .searchable(
            text: Binding(get: { store.searchText }, set: { store.setSearchText($0) }),
            placement: .navigationBarDrawer(displayMode: .always),
            prompt: "搜索名称与备注"
        )
        .onSubmit(of: .search) { store.submitSearch() }
        .toolbar { toolbar }
        .sheet(isPresented: $showFilter) {
            FilterSheet(store: store)
        }
        .sheet(isPresented: $showSettings) {
            SettingsView(context: context)
        }
        .sheet(item: Binding(get: { exporter.shareFile }, set: { exporter.shareFile = $0 })) {
            ShareSheet(url: $0.url)
        }
        .fullScreenCover(item: $previewTarget) { target in
            PreviewView(
                context: context, store: store, startIndex: target.index,
                showsInfo: target.showsInfo
            )
            .navigationTransition(.zoom(sourceID: target.key, in: zoomNamespace))
        }
        .hawkToast($exporter.toast)
        .alert(
            "操作失败",
            isPresented: Binding(
                get: { exporter.error != nil },
                set: { if !$0 { exporter.clearError() } }
            )
        ) {
            Button("好") { exporter.clearError() }
        } message: {
            Text(exporter.error ?? "")
        }
        .task { store.startIfNeeded() }
        .task(id: store.layoutVersion) {
            #if DEBUG
                // 本地冒烟：布局就绪后自动打开首图预览，用于检查预览与缩放布局
                let env = ProcessInfo.processInfo.environment
                let previewMode = env["HAWK_IOS_DEBUG_PREVIEW"]
                if previewTarget == nil, previewMode != nil, !store.displayItems.isEmpty {
                    openPreview(0, showsInfo: previewMode == "info")
                }
                if env["HAWK_IOS_DEBUG_SHEET"] == "filter" { showFilter = true }
                if env["HAWK_IOS_DEBUG_SHEET"] == "settings" { showSettings = true }
            #endif
        }
        .onChange(of: context.structureRevision) { _, _ in
            store.handleStructureChange()
        }
    }

    // MARK: - 工具栏

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        ToolbarItemGroup(placement: .topBarTrailing) {
            Menu {
                Section("排序") {
                    ForEach(SortChoice.all) { choice in
                        Button {
                            store.setSort(choice)
                        } label: {
                            if store.currentSort == choice {
                                Label(choice.label, systemImage: "checkmark")
                            } else {
                                Text(choice.label)
                            }
                        }
                    }
                }
                Section {
                    Button("筛选…", systemImage: "line.3.horizontal.decrease") {
                        showFilter = true
                    }
                    if store.hasActiveFilters {
                        Button("清除筛选", systemImage: "xmark.circle", role: .destructive) {
                            store.clearFilters()
                        }
                    }
                }
            } label: {
                Image(
                    systemName: store.hasActiveFilters
                        ? "line.3.horizontal.decrease.circle.fill"
                        : "line.3.horizontal.decrease.circle"
                )
            }
            .accessibilityLabel("排序与筛选")

            Menu {
                Button("刷新", systemImage: "arrow.clockwise") {
                    Task { await context.refreshAll() }
                }
                if store.view != .all {
                    Button("返回上层", systemImage: "chevron.up") {
                        store.navigateUp()
                    }
                }
                if let target = context.lockTarget(for: store.view),
                    context.isUnlocked(store.view)
                {
                    Button("重新锁定此\(target.dimension.title)", systemImage: "lock") {
                        context.relock(target)
                    }
                }
                Divider()
                Button("连接信息", systemImage: "info.circle") {
                    showSettings = true
                }
                Button("切换素材库", systemImage: "rectangle.2.swap") {
                    app.showConnectScreen()
                }
                Button("断开连接", systemImage: "xmark.circle", role: .destructive) {
                    app.disconnect()
                }
            } label: {
                Image(systemName: "ellipsis.circle")
            }
            .accessibilityLabel("更多")
        }
    }

    // MARK: - 动作

    private func openPreview(_ index: Int, showsInfo: Bool = false) {
        guard let item = store.item(at: index) else { return }
        previewTarget = PreviewTarget(index: index, key: item.key, showsInfo: showsInfo)
    }

    private func copyName(_ name: String) {
        UIPasteboard.general.string = name
        Haptics.success()
        exporter.toast = "已复制名称"
    }
}

struct PreviewTarget: Identifiable {
    let index: Int
    let key: String
    var showsInfo: Bool = false

    var id: String { "\(index)-\(key)" }
}

private struct ActiveFilterBar: View {
    let store: GridStore

    var body: some View {
        ScrollView(.horizontal) {
            HStack(spacing: 8) {
                if let star = store.query.star {
                    FilterChip(label: star == 0 ? "未评分" : "\(star) 星") {
                        store.applyFilters(
                            star: nil, color: store.query.color, size: store.query.size)
                    }
                }
                if let size = store.query.size {
                    FilterChip(label: size.label) {
                        store.applyFilters(
                            star: store.query.star, color: store.query.color, size: nil)
                    }
                }
                if let color = store.query.color {
                    FilterChip(label: color, tint: Color(hex: color), showsDot: true) {
                        store.applyFilters(
                            star: store.query.star, color: nil, size: store.query.size)
                    }
                }
                Button("清除全部") {
                    store.clearFilters()
                }
                .font(.footnote)
                .foregroundStyle(HawkTheme.fg1)
            }
            .padding(.horizontal, HawkTheme.screenPadding)
        }
        .scrollIndicators(.hidden)
        .padding(.vertical, 8)
        .background(HawkTheme.bg1)
    }
}
