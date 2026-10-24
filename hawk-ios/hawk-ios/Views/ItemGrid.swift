import SwiftUI
import UIKit

/// 远程图片：缩略图/原图都走 ImageStore 缓存与降采样。
/// 新入库素材的缩略图由后台异步生成：首次 404 时短暂重试，避免网格留下永久占位。
struct RemoteImage: View {
    let url: URL?
    var maxPixelSize: Int = 1024
    var contentMode: ContentMode = .fill

    @State private var image: UIImage?
    @State private var failed = false

    var body: some View {
        ZStack {
            Rectangle()
                .fill(HawkTheme.bg2)
                .overlay {
                    if failed {
                        Image(systemName: "photo")
                            .font(.title3)
                            .foregroundStyle(HawkTheme.fg1.opacity(0.5))
                    } else if image == nil {
                        ProgressView()
                            .controlSize(.small)
                            .tint(HawkTheme.fg1.opacity(0.6))
                    }
                }
            if let image {
                Image(uiImage: image)
                    .resizable()
                    .aspectRatio(contentMode: contentMode)
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
            for attempt in 0..<3 {
                let loaded =
                    if maxPixelSize <= 1024 {
                        await ImageStore.shared.thumbnail(for: url)
                    } else {
                        await ImageStore.shared.original(for: url, maxPixelSize: maxPixelSize)
                    }
                guard !Task.isCancelled else { return }
                if let loaded {
                    withAnimation(.easeOut(duration: 0.15)) { image = loaded }
                    return
                }
                if attempt < 2 {
                    try? await Task.sleep(nanoseconds: 1_500_000_000)
                    if Task.isCancelled { return }
                }
            }
            failed = true
        }
    }
}

/// 网格卡片：缩略图 + 名称 + 尺寸；长按出系统菜单，点击进预览。
struct ItemCard: View {
    @Environment(PreviewCoordinator.self) private var preview
    let cell: GridLayout.Cell
    let title: String
    let thumbnailURL: URL?
    let starCount: Int
    let metaHeight: CGFloat
    /// 预览打开期间隐藏缩略图：回位落进空格子（相册行为），否则图片会叠在同一个缩略图上
    var thumbHidden: Bool = false
    var onOpen: (CGRect?) -> Void = { _ in }
    var onSave: () -> Void = {}
    var onShare: () -> Void = {}
    var onInfo: () -> Void = {}
    var onCopyName: () -> Void = {}

    private var thumbnail: some View {
        // 用透明度隐藏而不是把 RemoteImage 移出层级：否则回场时视图重建会重新走一遍
        // 「占位 → 加载 → 显示」，落位瞬间会闪一下
        RemoteImage(url: thumbnailURL)
            .opacity(thumbHidden ? 0 : 1)
            .frame(width: cell.width, height: cell.height)
            .overlay(alignment: .topTrailing) {
                if !thumbHidden, starCount > 0 {
                    Label("\(starCount)", systemImage: "star.fill")
                        .labelStyle(.titleAndIcon)
                        .font(.caption2.weight(.semibold))
                        .foregroundStyle(HawkTheme.star)
                        .padding(.horizontal, 5)
                        .padding(.vertical, 3)
                        .background(.black.opacity(0.45), in: Capsule())
                        .padding(5)
                        .accessibilityLabel("\(starCount) 星")
                }
            }
    }

    var body: some View {
        GeometryReader { proxy in
            Button {
                // hero 的落点是缩略图区域（卡片上半部），不是含标题区的整卡
                let card = proxy.frame(in: .global)
                onOpen(CGRect(x: card.minX, y: card.minY, width: cell.width, height: cell.height))
            } label: {
                cardBody
            }
            .buttonStyle(.plain)
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("\(title)，\(cell.item.width) × \(cell.item.height)")
            .accessibilityHint("打开预览")
            .contextMenu {
                Button("打开预览", systemImage: "arrow.up.left.and.arrow.down.right") {
                    onOpen(nil)
                }
                Button("保存到相册", systemImage: "square.and.arrow.down", action: onSave)
                Button("分享原图", systemImage: "square.and.arrow.up", action: onShare)
                Button("素材信息", systemImage: "info.circle", action: onInfo)
                Divider()
                Button("复制名称", systemImage: "doc.on.doc", action: onCopyName)
            } preview: {
                RemoteImage(url: thumbnailURL)
                    .frame(width: min(cell.width * 1.6, 320), height: min(cell.height * 1.6, 320))
            }
        }
        .frame(width: cell.width, height: cell.height + metaHeight)
        .onGeometryChange(for: CGRect.self) {
            $0.frame(in: .global)
        } action: { frame in
            preview.reportCellFrame(
                CGRect(x: frame.minX, y: frame.minY, width: cell.width, height: cell.height),
                key: cell.item.key)
        }
    }

    private var cardBody: some View {
        VStack(spacing: 0) {
            thumbnail

            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(.caption)
                    .foregroundStyle(HawkTheme.fg0)
                    .lineLimit(2)
                    .truncationMode(.middle)
                    .multilineTextAlignment(.leading)
                    .frame(maxWidth: .infinity, alignment: .leading)
                Text(Format.dimensions(width: cell.item.width, height: cell.item.height))
                    .font(.caption2)
                    .foregroundStyle(HawkTheme.fg1)
                    .lineLimit(1)
            }
            .padding(.horizontal, 6)
            .padding(.vertical, 4)
            .frame(width: cell.width, height: metaHeight, alignment: .topLeading)
            .background(HawkTheme.bg2)
        }
        .frame(width: cell.width, height: cell.height + metaHeight)
        .background(HawkTheme.bg2)
        .clipShape(RoundedRectangle(cornerRadius: HawkTheme.cardRadius, style: .continuous))
        .contentShape(RoundedRectangle(cornerRadius: HawkTheme.cardRadius, style: .continuous))
    }
}

struct ItemGridView: View {
    @Environment(PreviewCoordinator.self) private var preview
    let context: LibraryContext
    let store: GridStore
    let targetHeight: CGFloat
    /// 打开预览：带上格子当前的全屏矩形（hero 转场起点）
    var onOpen: (Int, CGRect?) -> Void
    var onInfo: (Int) -> Void
    var onSave: (SkeletonItem) -> Void
    var onShare: (SkeletonItem, String) -> Void
    var onCopyName: (String) -> Void

    @ScaledMetric(relativeTo: .caption) private var metaHeight: CGFloat = 46
    @State private var rows: [GridLayout.Row] = []

    private struct LayoutKey: Hashable {
        let version: Int
        let width: Int
        let target: Int
        let meta: Int
    }

    var body: some View {
        GeometryReader { proxy in
            let contentWidth = max(1, proxy.size.width - HawkTheme.screenPadding * 2)
            ScrollViewReader { scroller in
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: HawkTheme.gridGap) {
                        ForEach(rows) { row in
                            HStack(alignment: .top, spacing: HawkTheme.gridGap) {
                                ForEach(row.cells) { cell in
                                    let detail = store.detail(for: cell.item)
                                    ItemCard(
                                        cell: cell,
                                        title: ItemKey.displayName(item: cell.item, detail: detail),
                                        thumbnailURL: context.api.thumbnailURL(id: cell.item.id),
                                        starCount: store.star(for: cell.item),
                                        metaHeight: metaHeight,
                                        thumbHidden: preview.hidesSource
                                            && preview.activeKey == cell.item.key,
                                        onOpen: { frame in onOpen(cell.index, frame) },
                                        onSave: { onSave(cell.item) },
                                        onShare: {
                                            onShare(
                                                cell.item,
                                                ItemKey.displayName(item: cell.item, detail: detail)
                                            )
                                        },
                                        onInfo: { onInfo(cell.index) },
                                        onCopyName: {
                                            onCopyName(
                                                ItemKey.displayName(item: cell.item, detail: detail)
                                            )
                                        }
                                    )
                                }
                                Spacer(minLength: 0)
                            }
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .task(id: store.skeletonVersion) {
                                store.ensureWindow(row.indexRange)
                            }
                            .id(row.id)
                        }
                    }
                    .padding(HawkTheme.screenPadding)
                    .scrollTargetLayout()
                }
                .scrollIndicators(.hidden)
                .contentMargins(.bottom, 56, for: .scrollContent)
                .scrollPosition(
                    id: Binding(
                        get: { store.scrollAnchor },
                        set: { store.updateScrollAnchor($0) }
                    ), anchor: .top
                )
                .refreshable {
                    await context.refreshAll()
                }
                .onChange(of: preview.scrollRequest) { _, key in
                    guard let key,
                        let row = rows.first(where: { $0.cells.contains { $0.item.key == key } })
                    else { return }
                    // 滚到屏幕中间（预览遮罩还在，用户看不到滚动过程）
                    scroller.scrollTo(row.id, anchor: .center)
                    preview.clearScrollRequest()
                }
                .task(
                    id: LayoutKey(
                        version: store.layoutVersion, width: Int(contentWidth),
                        target: Int(targetHeight), meta: Int(metaHeight))
                ) {
                    let items = store.displayItems
                    let width = contentWidth
                    let target = targetHeight
                    let meta = metaHeight
                    rows = GridLayout.rows(items, width: width, targetHeight: target)
                    // 首屏内容到达后把记住的位置还回去
                    _ = meta
                }
                .overlay(alignment: .top) {
                    if store.loading, !store.displayItems.isEmpty {
                        ProgressView()
                            .controlSize(.small)
                            .tint(HawkTheme.fg1)
                            .padding(.horizontal, 10)
                            .padding(.vertical, 6)
                            .background(.ultraThinMaterial, in: Capsule())
                            .padding(.top, 8)
                    }
                }
                .overlay {
                    if store.displayItems.isEmpty, !store.loading, !store.firstPageLoading,
                        store.lockedTarget == nil
                    {
                        emptyState
                    }
                }
            }
        }
        .background(HawkTheme.bg0)
    }

    private var emptyState: some View {
        ContentUnavailableView {
            Label(
                store.view == .trash ? "回收站为空" : "没有素材",
                systemImage: store.view == .trash ? "trash" : "photo.on.rectangle.angled")
        } description: {
            Text(store.emptyDescription)
        } actions: {
            if store.hasActiveFilters || !store.query.keywords.isEmpty {
                Button("清除筛选与搜索") {
                    store.searchText = ""
                    store.clearFilters(keepingSearch: false)
                    store.submitSearch()
                }
            }
        }
    }
}
