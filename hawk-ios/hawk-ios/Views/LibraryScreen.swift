import SwiftUI

struct LibraryScreen: View {
    @Environment(AppModel.self) private var app
    @Environment(\.scenePhase) private var scenePhase
    @AppStorage("hawk.ios.thumbSize") private var thumbSize = 200.0

    let store: LibraryStore
    let session: LibrarySession
    let showsSidebarButton: Bool

    @State private var showSidebar = false
    @State private var showFilter = false
    @State private var showSettings = false
    @State private var previewTarget: PreviewTarget?

    var body: some View {
        VStack(spacing: 0) {
            if session.isRemote {
                RemoteBanner(label: session.label) {
                    app.disconnect()
                }
            }
            if store.hasActiveFilters {
                ActiveFilterBar(store: store)
            }
            ItemGridView(store: store, targetHeight: thumbSize) { index in
                previewTarget = PreviewTarget(index: index)
            }
        }
        .background(HawkTheme.bg0)
        .navigationTitle(store.viewTitle)
        .navigationBarTitleDisplayMode(.inline)
        .searchable(
            text: Binding(get: { store.searchText }, set: { store.setSearchText($0) }),
            placement: .navigationBarDrawer(displayMode: .always),
            prompt: "搜索名称与备注"
        )
        .onSubmit(of: .search) {
            store.submitSearch()
        }
        .toolbar {
            if showsSidebarButton {
                ToolbarItem(placement: .topBarLeading) {
                    Button {
                        showSidebar = true
                    } label: {
                        Image(systemName: "sidebar.left")
                    }
                    .accessibilityLabel("打开导航")
                }
            }
            ToolbarItemGroup(placement: .topBarTrailing) {
                Button {
                    showFilter = true
                } label: {
                    Image(
                        systemName: store.hasActiveFilters
                            ? "line.3.horizontal.decrease.circle.fill"
                            : "line.3.horizontal.decrease.circle")
                }
                .accessibilityLabel("筛选")

                sortMenu
                moreMenu
            }
        }
        .sheet(isPresented: $showSidebar) {
            NavigationStack {
                SidebarView(store: store, session: session) {
                    showSidebar = false
                }
                .toolbar {
                    ToolbarItem(placement: .topBarLeading) {
                        Button("关闭") { showSidebar = false }
                    }
                }
            }
            .presentationDetents([.medium, .large])
            .presentationDragIndicator(.visible)
            .preferredColorScheme(.dark)
        }
        .sheet(isPresented: $showFilter) {
            FilterSheet(store: store)
                .preferredColorScheme(.dark)
        }
        .sheet(isPresented: $showSettings) {
            SettingsView(session: session)
                .preferredColorScheme(.dark)
        }
        .fullScreenCover(item: $previewTarget) { target in
            PreviewView(store: store, startIndex: target.index)
        }
        .onChange(of: scenePhase) { _, phase in
            guard phase == .active, previewTarget == nil else { return }
            Task { await store.refreshIfStale() }
        }
        .task(id: store.skeletonVersion) {
            #if DEBUG
                if previewTarget == nil,
                    ProcessInfo.processInfo.environment["HAWK_IOS_DEBUG_PREVIEW"] == "1",
                    !store.skeleton.isEmpty
                {
                    previewTarget = PreviewTarget(index: 0)
                }
            #endif
        }
    }

    private var sortMenu: some View {
        Menu {
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
        } label: {
            Image(systemName: "arrow.up.arrow.down")
        }
        .accessibilityLabel("排序")
    }

    private var moreMenu: some View {
        Menu {
            if store.view != .all {
                Button("返回上级", systemImage: "chevron.up") {
                    store.navigateUp()
                }
                Divider()
            }
            Button("刷新素材库", systemImage: "arrow.clockwise") {
                Task { await store.refreshAll() }
            }
            Button("连接信息", systemImage: "info.circle") {
                showSettings = true
            }
        } label: {
            Image(systemName: "ellipsis.circle")
        }
        .accessibilityLabel("更多")
    }
}

private struct PreviewTarget: Identifiable {
    let index: Int
    var id: Int { index }
}

private struct RemoteBanner: View {
    let label: String
    let disconnect: () -> Void

    var body: some View {
        HStack(spacing: 7) {
            Image(systemName: "globe")
                .font(.system(size: 11, weight: .semibold))
            Text("当前浏览：\(label)")
                .font(.system(size: 11))
                .lineLimit(1)
            Spacer(minLength: 4)
            Button(action: disconnect) {
                Image(systemName: "xmark")
                    .font(.system(size: 10, weight: .bold))
                    .frame(width: 22, height: 22)
                    .contentShape(Rectangle())
            }
            .accessibilityLabel("断开远程连接")
        }
        .foregroundStyle(HawkTheme.fg1)
        .padding(.horizontal, 12)
        .padding(.vertical, 5)
        .background(HawkTheme.bg1)
        .overlay(alignment: .bottom) {
            Rectangle().fill(HawkTheme.border).frame(height: 1)
        }
    }
}

private struct ActiveFilterBar: View {
    let store: LibraryStore

    var body: some View {
        ScrollView(.horizontal) {
            HStack(spacing: 8) {
                if let star = store.query.star {
                    FilterChip(label: "\(star) 星", color: HawkTheme.star) {
                        store.setStar(nil)
                    }
                }
                if let size = store.query.size {
                    FilterChip(label: size.label, color: HawkTheme.accent) {
                        store.setSize(nil)
                    }
                }
                if let color = store.query.color {
                    FilterChip(label: color, color: Color(hex: color), showsDot: true) {
                        store.setColor(nil)
                    }
                }
                Button("清除全部") {
                    store.clearFilters()
                }
                .font(.system(size: 11))
                .foregroundStyle(HawkTheme.fg1)
            }
            .padding(.horizontal, 12)
        }
        .scrollIndicators(.hidden)
        .padding(.vertical, 6)
        .background(HawkTheme.bg1)
        .overlay(alignment: .bottom) {
            Rectangle().fill(HawkTheme.border).frame(height: 1)
        }
    }
}

private struct FilterChip: View {
    let label: String
    let color: Color
    var showsDot = false
    let clear: () -> Void

    var body: some View {
        HStack(spacing: 5) {
            if showsDot {
                Circle()
                    .fill(color)
                    .frame(width: 10, height: 10)
                    .overlay(Circle().stroke(Color.white.opacity(0.2), lineWidth: 1))
            } else {
                Circle()
                    .fill(color)
                    .frame(width: 6, height: 6)
            }
            Text(label)
                .font(.system(size: 11))
            Button(action: clear) {
                Image(systemName: "xmark")
                    .font(.system(size: 8, weight: .bold))
            }
            .accessibilityLabel("清除此筛选")
        }
        .foregroundStyle(HawkTheme.fg0)
        .padding(.horizontal, 9)
        .padding(.vertical, 4)
        .background(HawkTheme.bg3, in: Capsule())
    }
}
