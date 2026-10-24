import SwiftUI

/// 浏览页：智能分组 / 文件夹树 / 分类 / 标签。
/// iPhone 是「浏览」Tab 的根（点击推入网格），iPad 是分栏侧栏（点击换 detail）。
struct BrowseScreen: View {
    let context: LibraryContext
    var active: LibraryView?
    let onSelect: (LibraryView) -> Void

    @State private var expandedFolders: Set<String> = []
    @State private var showSettings = false

    var body: some View {
        List {
            if context.liveState == .reconnecting {
                Section {
                    Label("正在重新连接素材库…", systemImage: "wifi.exclamationmark")
                        .font(.footnote)
                        .foregroundStyle(HawkTheme.fg1)
                }
            }

            Section("智能分组") {
                self.smartRow(.all)
                self.smartRow(.root)
                self.smartRow(.uncategorized)
                self.smartRow(.untagged)
                self.smartRow(.trash)
            }

            Section("文件夹") {
                if context.taxonomyLoading && context.folders == nil {
                    placeholderRow("正在读取文件夹…")
                } else if let children = context.folders?.children, !children.isEmpty {
                    ForEach(children) { node in
                        FolderRow(
                            context: context,
                            node: node,
                            depth: 0,
                            active: active,
                            expanded: $expandedFolders,
                            onSelect: onSelect
                        )
                    }
                } else {
                    placeholderRow("暂无文件夹")
                }
            }

            Section("分类") {
                if context.taxonomyLoading && context.categories.isEmpty {
                    placeholderRow("正在读取分类…")
                } else if context.categories.isEmpty {
                    placeholderRow("暂无分类")
                } else {
                    ForEach(context.categories) { category in
                        let view = LibraryView.category(category.name)
                        BrowseRow(
                            view: view,
                            title: category.name,
                            count: category.count,
                            icon: "square.grid.2x2",
                            locked: context.categoryIsLocked(category.name),
                            unlocked: context.isUnlocked(category: category.name),
                            isActive: active == view,
                            onRelock: context.isUnlocked(category: category.name)
                                ? {
                                    context.relock(
                                        LockTarget(dimension: .category, name: category.name))
                                }
                                : nil
                        ) {
                            onSelect(view)
                        }
                    }
                }
            }

            Section("标签") {
                if context.taxonomyLoading && context.tags.isEmpty {
                    placeholderRow("正在读取标签…")
                } else if context.tags.isEmpty {
                    placeholderRow("暂无标签")
                } else {
                    ForEach(context.tags) { tag in
                        let view = LibraryView.tag(tag.name)
                        BrowseRow(
                            view: view,
                            title: tag.name,
                            count: tag.count,
                            icon: "tag",
                            locked: context.tagIsLocked(tag.name),
                            unlocked: context.isUnlocked(tag: tag.name),
                            isActive: active == view,
                            onRelock: context.isUnlocked(tag: tag.name)
                                ? { context.relock(LockTarget(dimension: .tag, name: tag.name)) }
                                : nil
                        ) {
                            onSelect(view)
                        }
                    }
                }
            }

            Section {
                Button {
                    showSettings = true
                } label: {
                    Label("连接信息", systemImage: "info.circle")
                }
                if let label = context.scanningLabel {
                    HStack(spacing: 8) {
                        ProgressView().controlSize(.small)
                        Text(label)
                            .font(.footnote)
                            .foregroundStyle(HawkTheme.fg1)
                    }
                }
            } footer: {
                Text(
                    "\(context.title)\n\(context.isRemote ? "广域网" : "局域网直连") · \(context.accessLabel)"
                )
                .font(.caption)
                .foregroundStyle(HawkTheme.fg1)
            }
        }
        .listStyle(.insetGrouped)
        .contentMargins(.bottom, 52, for: .scrollContent)
        .scrollContentBackground(.hidden)
        .background(HawkTheme.bg0)
        .navigationTitle("素材库")
        .navigationBarTitleDisplayMode(.large)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button {
                    showSettings = true
                } label: {
                    Image(systemName: "info.circle")
                }
                .accessibilityLabel("连接信息")
            }
        }
        .sheet(isPresented: $showSettings) {
            SettingsView(context: context)
        }
        .onAppear { expandAncestors(of: active) }
        .onChange(of: active) { _, value in expandAncestors(of: value) }
    }

    private func smartRow(_ view: LibraryView) -> some View {
        BrowseRow(
            view: view,
            title: view.title,
            count: context.countsLoading ? nil : context.counts.count(for: view),
            icon: view.icon,
            locked: false,
            unlocked: false,
            isActive: active == view
        ) {
            onSelect(view)
        }
    }

    private func placeholderRow(_ text: String) -> some View {
        Text(text)
            .font(.footnote)
            .foregroundStyle(HawkTheme.fg1)
    }

    private func expandAncestors(of view: LibraryView?) {
        guard case .folder(let path) = view else { return }
        var current = path
        while let slash = current.lastIndex(of: "/") {
            current = String(current[..<slash])
            expandedFolders.insert(current)
        }
    }
}

// MARK: - 行

private struct BrowseRow: View {
    let view: LibraryView
    let title: String
    let count: Int?
    let icon: String
    let locked: Bool
    let unlocked: Bool
    let isActive: Bool
    var indent: CGFloat = 0
    var onRelock: (() -> Void)?
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 10) {
                Image(systemName: icon)
                    .font(.subheadline)
                    .foregroundStyle(isActive ? HawkTheme.accent : HawkTheme.fg1)
                    .frame(width: 22)
                Text(title)
                    .font(.body)
                    .foregroundStyle(HawkTheme.fg0)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Spacer(minLength: 8)
                if locked { LockBadge(unlocked: unlocked) }
                CountBadge(value: count)
                if isActive {
                    Image(systemName: "checkmark")
                        .font(.footnote.weight(.semibold))
                        .foregroundStyle(HawkTheme.accent)
                }
            }
            .padding(.leading, indent)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .listRowBackground(isActive ? HawkTheme.bg3 : HawkTheme.bg1)
        .accessibilityAddTraits(isActive ? [.isSelected] : [])
        .contextMenu {
            if let onRelock {
                Button("重新锁定", systemImage: "lock") {
                    Haptics.tap()
                    onRelock()
                }
            }
        }
    }
}

private struct FolderRow: View {
    let context: LibraryContext
    let node: FolderNode
    let depth: Int
    let active: LibraryView?
    @Binding var expanded: Set<String>
    let onSelect: (LibraryView) -> Void

    private var isExpanded: Bool { expanded.contains(node.path) }

    private var relockAction: (() -> Void)? {
        guard context.isUnlocked(folder: node.path),
            let target = LockCoverage.target(for: .folder(node.path), in: context.locks)
        else { return nil }
        return { context.relock(target) }
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 0) {
                Button {
                    withAnimation(.easeInOut(duration: 0.18)) {
                        if isExpanded {
                            expanded.remove(node.path)
                        } else {
                            expanded.insert(node.path)
                        }
                    }
                } label: {
                    Image(systemName: "chevron.right")
                        .font(.caption2.weight(.bold))
                        .foregroundStyle(HawkTheme.fg1)
                        .rotationEffect(.degrees(isExpanded ? 90 : 0))
                        .frame(width: 26, height: 32)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .opacity(node.children.isEmpty ? 0 : 1)
                .disabled(node.children.isEmpty)
                .accessibilityLabel(isExpanded ? "折叠子文件夹" : "展开子文件夹")

                BrowseRow(
                    view: .folder(node.path),
                    title: node.name,
                    count: node.count,
                    icon: "folder",
                    locked: context.folderIsLocked(node.path),
                    unlocked: context.isUnlocked(folder: node.path),
                    isActive: active == .folder(node.path),
                    onRelock: relockAction
                ) {
                    onSelect(.folder(node.path))
                }
            }
            .padding(.leading, CGFloat(depth) * 14)

            if isExpanded {
                ForEach(node.children) { child in
                    FolderRow(
                        context: context,
                        node: child,
                        depth: depth + 1,
                        active: active,
                        expanded: $expanded,
                        onSelect: onSelect
                    )
                }
            }
        }
        .listRowInsets(EdgeInsets(top: 0, leading: 8, bottom: 0, trailing: 16))
    }
}
