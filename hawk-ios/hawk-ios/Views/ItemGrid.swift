import SwiftUI
import UIKit

struct RemoteImage: View {
    let url: URL?
    var maxPixelSize: Int = 1024
    var contentMode: ContentMode = .fill

    @State private var image: UIImage?
    @State private var failed = false

    var body: some View {
        ZStack {
            if let image {
                Image(uiImage: image)
                    .resizable()
                    .aspectRatio(contentMode: contentMode)
            } else {
                Rectangle()
                    .fill(HawkTheme.bg2)
                    .overlay {
                        if failed {
                            Image(systemName: "photo")
                                .font(.system(size: 18))
                                .foregroundStyle(HawkTheme.fg1.opacity(0.5))
                        }
                    }
            }
        }
        .clipped()
        .task(id: url) {
            image = nil
            failed = false
            guard let url else {
                failed = true
                return
            }
            // 新入库素材的缩略图由后台异步生成：首次 404 时短暂重试，避免网格留下永久占位。
            for attempt in 0..<4 {
                let loaded =
                    if maxPixelSize <= 1024 {
                        await ImageStore.shared.thumbnail(for: url)
                    } else {
                        await ImageStore.shared.original(for: url, maxPixelSize: maxPixelSize)
                    }
                guard !Task.isCancelled else { return }
                if let loaded {
                    image = loaded
                    return
                }
                if attempt < 3 {
                    try? await Task.sleep(nanoseconds: 1_500_000_000)
                    if Task.isCancelled { return }
                }
            }
            failed = true
        }
    }
}

struct ItemCard: View {
    let cell: GridLayout.Cell
    let detail: Item?
    let thumbnailURL: URL?
    let onOpen: () -> Void

    private var title: String {
        if let detail { return detail.displayName }
        let name = ItemKey.baseName(path: cell.item.path)
        let ext = ItemKey.ext(path: cell.item.path)
        return ext.isEmpty ? name : "\(name).\(ext)"
    }

    var body: some View {
        Button(action: onOpen) {
            VStack(spacing: 0) {
                RemoteImage(url: thumbnailURL)
                    .frame(width: cell.width, height: cell.height)
                    .overlay(alignment: .topTrailing) {
                        if cell.item.star > 0 {
                            Text("★\(cell.item.star)")
                                .font(.system(size: 11, weight: .semibold))
                                .foregroundStyle(HawkTheme.star)
                                .padding(.horizontal, 4)
                                .padding(.top, 4)
                                .shadow(color: .black.opacity(0.8), radius: 2, y: 1)
                        }
                    }

                VStack(spacing: 2) {
                    Text(title)
                        .font(.system(size: 12))
                        .foregroundStyle(HawkTheme.fg0)
                        .lineLimit(2)
                        .multilineTextAlignment(.center)
                        .frame(maxWidth: .infinity, alignment: .top)
                    Text(Format.dimensions(width: cell.item.width, height: cell.item.height))
                        .font(.system(size: 11))
                        .foregroundStyle(HawkTheme.fg1)
                }
                .padding(.horizontal, 6)
                .padding(.top, 5)
                .frame(width: cell.width, height: GridLayout.metaHeight, alignment: .top)
                .background(HawkTheme.bg2)
            }
            .frame(width: cell.width, height: cell.height + GridLayout.metaHeight)
            .background(HawkTheme.bg2)
            .clipShape(RoundedRectangle(cornerRadius: 4, style: .continuous))
            .contentShape(RoundedRectangle(cornerRadius: 4, style: .continuous))
        }
        .buttonStyle(.plain)
        .accessibilityLabel(title)
        .accessibilityHint("打开预览")
    }
}

struct ItemGridView: View {
    let store: LibraryStore
    let targetHeight: CGFloat
    let onOpen: (Int) -> Void

    @State private var rows: [GridLayout.Row] = []

    private struct LayoutKey: Hashable {
        let version: Int
        let width: Int
        let target: Int
    }

    var body: some View {
        GeometryReader { proxy in
            let contentWidth = max(1, proxy.size.width - 24)
            ScrollView {
                LazyVStack(alignment: .leading, spacing: GridLayout.gap) {
                    ForEach(rows) { row in
                        HStack(alignment: .top, spacing: GridLayout.gap) {
                            ForEach(row.cells) { cell in
                                ItemCard(
                                    cell: cell,
                                    detail: store.details[cell.item.key],
                                    thumbnailURL: store.api.thumbnailURL(id: cell.item.id)
                                ) {
                                    onOpen(cell.index)
                                }
                            }
                            Spacer(minLength: 0)
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .task(id: store.skeletonVersion) {
                            store.ensureWindow(row.indexRange)
                        }
                    }
                }
                .padding(12)
            }
            .scrollIndicators(.hidden)
            .refreshable {
                await store.refreshAll()
            }
            .task(
                id: LayoutKey(
                    version: store.layoutVersion, width: Int(contentWidth),
                    target: Int(targetHeight))
            ) {
                let items = store.displayItems
                let width = contentWidth
                let target = targetHeight
                rows = GridLayout.rows(items, width: width, targetHeight: target)
            }
            .overlay(alignment: .top) {
                if store.loading, !store.displayItems.isEmpty {
                    ProgressView()
                        .tint(HawkTheme.fg1)
                        .padding(8)
                        .background(HawkTheme.bg1.opacity(0.9), in: Capsule())
                        .padding(.top, 8)
                }
            }
            .overlay {
                if store.displayItems.isEmpty {
                    if store.loading || store.firstPageLoading {
                        loadingGrid
                    } else if store.error == nil {
                        emptyState
                    }
                }
            }
            .overlay(alignment: .top) {
                if let error = store.error {
                    ErrorBanner(message: error)
                        .padding(12)
                }
            }
        }
        .background(HawkTheme.bg0)
    }

    private var loadingGrid: some View {
        ScrollView {
            LazyVGrid(columns: [GridItem(.adaptive(minimum: 144), spacing: 10)], spacing: 10) {
                ForEach(0..<12, id: \.self) { index in
                    RoundedRectangle(cornerRadius: 4, style: .continuous)
                        .fill(HawkTheme.bg2)
                        .frame(height: [190.0, 150.0, 230.0][index % 3])
                }
            }
            .padding(12)

            HStack(spacing: 8) {
                ProgressView()
                    .controlSize(.small)
                    .tint(HawkTheme.fg1)
                Text("正在加载素材…")
                    .font(.system(size: 12))
                    .foregroundStyle(HawkTheme.fg1)
            }
            .padding(.bottom, 24)
        }
        .scrollIndicators(.hidden)
        .background(HawkTheme.bg0)
    }

    private var emptyState: some View {
        ContentUnavailableView {
            Label(
                store.view == .trash ? "回收站为空" : "暂无素材",
                systemImage: store.view == .trash ? "trash" : "photo.on.rectangle.angled")
        } description: {
            if store.hasActiveFilters {
                Text("当前筛选条件下没有素材，试试清除筛选。")
            } else if !store.query.keywords.isEmpty {
                Text("没有匹配「\(store.query.keywords.joined(separator: " "))」的素材")
            } else {
                Text("在桌面端导入素材后，这里会自动出现。")
            }
        } actions: {
            if store.hasActiveFilters || !store.query.keywords.isEmpty {
                Button("清除筛选与搜索") {
                    store.clearFilters()
                    store.searchText = ""
                    store.submitSearch()
                }
            }
        }
    }
}
