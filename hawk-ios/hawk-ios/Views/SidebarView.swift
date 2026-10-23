import SwiftUI

struct SidebarView: View {
    let store: LibraryStore
    let session: LibrarySession
    var onNavigate: (() -> Void)?

    @State private var foldersCollapsed = false
    @State private var categoriesCollapsed = false
    @State private var tagsCollapsed = false

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 0) {
                header
                    .padding(.horizontal, 12)
                    .padding(.top, 12)
                    .padding(.bottom, 10)

                smartEntries

                sectionHeader("文件夹", collapsed: $foldersCollapsed)
                if !foldersCollapsed {
                    let children = store.folders?.children ?? []
                    if store.taxonomyLoading {
                        SectionHint(text: "加载中…")
                    } else if children.isEmpty {
                        SectionHint(text: "暂无文件夹")
                    } else {
                        ForEach(children) { node in
                            FolderRow(
                                node: node, depth: 0, activePath: activeFolderPath, store: store,
                                onNavigate: onNavigate)
                        }
                    }
                }

                sectionHeader("分类", collapsed: $categoriesCollapsed)
                if !categoriesCollapsed {
                    if store.taxonomyLoading {
                        SectionHint(text: "加载中…")
                    } else if store.categories.isEmpty {
                        SectionHint(text: "暂无分类")
                    } else {
                        ForEach(store.categories) { category in
                            SidebarRow(
                                title: category.name,
                                count: category.count,
                                active: store.view == .category(category.name),
                                icon: "square.grid.2x2",
                                iconSize: 11
                            ) {
                                store.select(view: .category(category.name))
                                onNavigate?()
                            }
                        }
                    }
                }

                sectionHeader("标签", collapsed: $tagsCollapsed)
                if !tagsCollapsed {
                    if store.taxonomyLoading {
                        SectionHint(text: "加载中…")
                    } else if store.tags.isEmpty {
                        SectionHint(text: "暂无标签")
                    } else {
                        ForEach(store.tags) { tag in
                            SidebarRow(
                                title: tag.name,
                                count: tag.count,
                                active: store.view == .tag(tag.name),
                                icon: "tag",
                                iconSize: 11
                            ) {
                                store.select(view: .tag(tag.name))
                                onNavigate?()
                            }
                        }
                    }
                }

                Color.clear.frame(height: 20)
            }
        }
        .scrollIndicators(.hidden)
        .background(HawkTheme.bg1)
        .navigationTitle("素材库")
        .navigationBarTitleDisplayMode(.inline)
    }

    private var activeFolderPath: String? {
        if case .folder(let path) = store.view { return path }
        return nil
    }

    private var header: some View {
        HStack(spacing: 9) {
            HawkMark(size: 28)
            VStack(alignment: .leading, spacing: 1) {
                Text(store.library?.name ?? "hawk")
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundStyle(HawkTheme.fg0)
                    .lineLimit(1)
                HStack(spacing: 4) {
                    Text(session.isRemote ? "远程" : "局域网")
                    Text("·")
                    Text(session.accessLabel)
                }
                .font(.system(size: 10))
                .foregroundStyle(HawkTheme.fg1)
                .lineLimit(1)
            }
            Spacer(minLength: 0)
        }
    }

    private var smartEntries: some View {
        VStack(spacing: 1) {
            SidebarRow(
                title: "全部素材", count: store.countsLoading ? nil : store.counts.all,
                active: store.view == .all, icon: "square.grid.2x2"
            ) {
                store.select(view: .all)
                onNavigate?()
            }
            SidebarRow(
                title: "根目录素材", count: store.countsLoading ? nil : store.counts.root,
                active: store.view == .root, icon: "house"
            ) {
                store.select(view: .root)
                onNavigate?()
            }
            SidebarRow(
                title: "未分类素材", count: store.countsLoading ? nil : store.counts.uncategorized,
                active: store.view == .uncategorized, icon: "tray"
            ) {
                store.select(view: .uncategorized)
                onNavigate?()
            }
            SidebarRow(
                title: "未标签素材", count: store.countsLoading ? nil : store.counts.untagged,
                active: store.view == .untagged, icon: "tag.slash"
            ) {
                store.select(view: .untagged)
                onNavigate?()
            }
            SidebarRow(
                title: "回收站", count: store.countsLoading ? nil : store.counts.trash,
                active: store.view == .trash, icon: "trash"
            ) {
                store.select(view: .trash)
                onNavigate?()
            }
        }
        .padding(.bottom, 6)
    }

    private func sectionHeader(_ title: String, collapsed: Binding<Bool>) -> some View {
        Button {
            withAnimation(.easeInOut(duration: 0.15)) {
                collapsed.wrappedValue.toggle()
            }
        } label: {
            HStack(spacing: 4) {
                Image(systemName: "chevron.right")
                    .font(.system(size: 9, weight: .bold))
                    .rotationEffect(.degrees(collapsed.wrappedValue ? 0 : 90))
                Text(title)
                    .font(.system(size: 11, weight: .semibold))
                Spacer()
            }
            .foregroundStyle(HawkTheme.fg1)
            .padding(.horizontal, 12)
            .padding(.top, 12)
            .padding(.bottom, 4)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

private struct SectionHint: View {
    let text: String

    var body: some View {
        Text(text)
            .font(.system(size: 11))
            .foregroundStyle(HawkTheme.fg1)
            .padding(.horizontal, 14)
            .padding(.vertical, 4)
    }
}

struct SidebarRow: View {
    let title: String
    var count: Int? = nil
    let active: Bool
    var icon: String? = nil
    var iconSize: CGFloat = 13
    var leadingIndent: CGFloat = 12
    var leadingAccessoryWidth: CGFloat = 0
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 7) {
                if leadingAccessoryWidth > 0 {
                    Color.clear.frame(width: leadingAccessoryWidth, height: 1)
                }
                if let icon {
                    Image(systemName: icon)
                        .font(.system(size: iconSize))
                        .frame(width: 16)
                        .foregroundStyle(active ? HawkTheme.fg0 : HawkTheme.fg1)
                }
                Text(title)
                    .font(.system(size: 13))
                    .lineLimit(1)
                    .foregroundStyle(active ? Color.white : HawkTheme.fg0)
                Spacer(minLength: 6)
                if let count {
                    Text("\(count)")
                        .font(.system(size: 11, weight: active ? .semibold : .regular))
                        .foregroundStyle(active ? Color.white : HawkTheme.fg1)
                        .monospacedDigit()
                }
            }
            .padding(.leading, leadingIndent)
            .padding(.trailing, 12)
            .padding(.vertical, 6)
            .contentShape(Rectangle())
            .background(active ? HawkTheme.bg3 : Color.clear)
        }
        .buttonStyle(.plain)
    }
}

private struct FolderRow: View {
    let node: FolderNode
    let depth: Int
    let activePath: String?
    let store: LibraryStore
    var onNavigate: (() -> Void)?

    @State private var expanded = false

    var body: some View {
        VStack(spacing: 1) {
            HStack(spacing: 1) {
                Button {
                    withAnimation(.easeInOut(duration: 0.15)) {
                        expanded.toggle()
                    }
                } label: {
                    Image(systemName: "chevron.right")
                        .font(.system(size: 9, weight: .bold))
                        .foregroundStyle(HawkTheme.fg1)
                        .rotationEffect(.degrees(expanded ? 90 : 0))
                        .frame(width: 18, height: 24)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .opacity(node.children.isEmpty ? 0 : 1)
                .disabled(node.children.isEmpty)

                SidebarRow(
                    title: node.name,
                    count: node.count,
                    active: activePath == node.path,
                    icon: "folder",
                    leadingIndent: 0
                ) {
                    store.select(view: .folder(node.path))
                    onNavigate?()
                }
            }
            .padding(.leading, CGFloat(depth) * 14 + 4)

            if expanded {
                ForEach(node.children) { child in
                    FolderRow(
                        node: child, depth: depth + 1, activePath: activePath, store: store,
                        onNavigate: onNavigate)
                }
            }
        }
    }
}
