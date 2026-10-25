import SwiftUI
import UIKit

/// 全屏预览（相册式）：横向翻页跟手、双击/双指缩放、放大后单指平移，未放大时下拉关闭；
/// 顶部标题与底部玻璃工具条（保存/分享/信息）。
///
/// 为什么自绘轮播而不是 ScrollView 分页：iOS 26 起放在滚动内容里的 DragGesture 会取消 ScrollView 的 pan
/// （rdar://FB10091372），而翻页与下拉关闭需要在同一个手势里按主轴锁定。
/// 这里用三段式画面（上一张/当前/下一张）+ 单个 DragGesture 编排：
///   - 未放大：水平拖动 = 轮播位移（首尾页橡皮筋抵抗），竖直拖动 = 下拉关闭
///   - 放大：缩放与平移交给当前页的 UIScrollView（ZoomableImage），外层手势整体让出
/// 手势轴在拖动开始时锁定一次（水平/竖直），之后不再改判，避免相册里常见的抖动。
struct PreviewView: View {
    /// 根层 overlay 的呈现协调器（关闭时清空 session）
    @Environment(PreviewCoordinator.self) private var coordinator
    let session: PreviewCoordinator.Session
    private var context: LibraryContext { session.context }
    private var store: GridStore { session.store }
    @State private var index: Int
    @State private var showsChrome = true
    @State private var showsInfo: Bool
    @State private var detail: Item?
    /// hero 进度：0 = 停在源格子，1 = 全屏（入场/回场都由它驱动）
    @State private var present: CGFloat = 0
    @State private var exporter = MediaExporter()

    // 当前页的缩放层：收场前归零，缩放状态用于让出外层单指手势
    @State private var zoom = ZoomController()
    @State private var zoomed = false

    // 手势过程态
    @State private var dragMode: DragMode = .idle
    @State private var pageDrag: CGFloat = 0
    @State private var dismissTranslation: CGFloat = 0
    @State private var pageAnimating = false

    /// 拖动模式：拖动开始时按主轴锁定
    private enum DragMode { case idle, page, dismiss }

    /// 轮播翻页阈值（视口宽度比例）；下拉关闭阈值：位移到这条线即判定关闭（scrim 同时刚好完全透明）
    private static let dismissDistance: CGFloat = 160
    private static let pageDistanceRatio: CGFloat = 0.18
    /// 甩动翻页阈值（预测位移占视口宽度的比例）：位移不够时靠速度也能翻，接近相册
    private static let pageFlickRatio: CGFloat = 0.45

    init(session: PreviewCoordinator.Session) {
        self.session = session
        _index = State(initialValue: session.index)
        _showsInfo = State(initialValue: session.showsInfo)
    }

    private var items: [SkeletonItem] { store.displayItems }
    private var currentItem: SkeletonItem? { store.item(at: index) }
    private var title: String {
        guard let item = currentItem else { return "" }
        return ItemKey.displayName(item: item, detail: detail)
    }

    /// 下拉进度（0…1）：按关闭阈值归一，手感和「拖到底 = 背景全透明」对齐
    private var dismissProgress: CGFloat {
        min(1, max(0, dismissTranslation / Self.dismissDistance))
    }

    private var scrimOpacity: Double { Double(present) * (1 - Double(dismissProgress)) }

    /// chrome 比 scrim 收得更快：开始下拉就淡出，避免文字压在半透明背景上
    private var chromeOpacity: Double { Double(present) * Double(max(0, 1 - dismissProgress * 2)) }

    /// 内容随下拉收向缩略图：到关闭阈值时缩到 60%（按需求调大「往回收」的幅度）
    private var contentScale: CGFloat { 1 - 0.4 * dismissProgress }

    /// 相邻页之间的固定间距（只拉开页与页的步距，不改变图片尺寸，翻页时不会忽大忽小）
    private static let pageGap: CGFloat = 14

    /// 缩小的同时逐渐带上卡片圆角，和缩略图形态衔接
    private var contentCornerRadius: CGFloat { 14 * dismissProgress }

    var body: some View {
        GeometryReader { proxy in
            let size = proxy.size
            let hero = heroTransform(size: size, origin: proxy.frame(in: .global).origin)
            ZStack {
                // 背后的网格是真实视图（预览在根层 overlay，不是 modal）：scrim 渐隐即可露出
                Color.black
                    .opacity(scrimOpacity)
                    .ignoresSafeArea()

                carousel(size: size)
                    // 宽高分别插值：终点必然与格子逐像素重合（单一比例缩放遇到留白/方向不一致就会偏）
                    .scaleEffect(
                        x: hero.scaleX * contentScale, y: hero.scaleY * contentScale
                    )
                    .clipShape(
                        RoundedRectangle(
                            cornerRadius: contentCornerRadius, style: .continuous)
                    )
                    .offset(
                        x: hero.offset.width,
                        y: hero.offset.height + dismissTranslation
                    )

                if showsChrome {
                    VStack(spacing: 0) {
                        topBar
                        Spacer(minLength: 0)
                    }
                    .opacity(chromeOpacity)
                    .transition(.opacity)
                }
            }
            .overlay(alignment: .bottom) {
                if showsChrome {
                    bottomBar
                        .opacity(chromeOpacity)
                        .transition(.move(edge: .bottom).combined(with: .opacity))
                }
            }
            .contentShape(Rectangle())
            // 放大时整体让出单指手势：平移与缩放交给当前页的 UIScrollView
            .gesture(dragGesture(size: size), including: zoomed ? .none : .all)
        }
        .onAppear {
            coordinator.setActiveKey(store.item(at: index)?.key)
            prefetchNeighbours(index)
            withAnimation(.spring(response: 0.36, dampingFraction: 0.86)) { present = 1 }
        }
        .onChange(of: index) { _, _ in
            // 翻页即同步：源格子置空跟着当前图；同时让网格先把该行滚进视口，
            // 松手时无需再滚动（否则会看到背景跳一下）
            let key = store.item(at: index)?.key
            coordinator.setActiveKey(key)
            if let key { coordinator.requestCenterScroll(key) }
            prefetchNeighbours(index)
        }
        .statusBarHidden(!showsChrome)
        .task(id: index) { await loadDetail(for: index) }
        .task {
            guard showsInfo else { return }
            if detail == nil { await loadDetail(for: index) }
            // 信息面板在预览出现后再推，避免和缩放转场同时发生
            try? await Task.sleep(nanoseconds: 400_000_000)
            showsInfo = true
        }
        .sheet(isPresented: $showsInfo) {
            ItemInfoSheet(item: detail, fallback: currentItem, context: context)
        }
        .sheet(item: Binding(get: { exporter.shareFile }, set: { exporter.shareFile = $0 })) {
            ShareSheet(url: $0.url)
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
    }

    /// hero 转场：present=1 时是全屏原始状态；present=0 时把当前图片缩到源格子大小并平移过去。
    /// 无源格子（右键菜单打开）退化为轻微缩放淡入。
    /// hero 落点：宽高分别插值到源格子矩形，避免「单一比例缩放 + 留白」算偏；
    /// origin 用 overlay 的全局原点，抵消安全区造成的坐标偏移（否则会整体偏状态栏高度）。
    private func heroTransform(size: CGSize, origin: CGPoint)
        -> (scaleX: CGFloat, scaleY: CGFloat, offset: CGSize)
    {
        let currentKey = store.item(at: index)?.key
        let frame = currentKey.flatMap { coordinator.cellFrames[$0] } ?? session.sourceFrame
        guard let frame, frame.width > 1, size.width > 1,
            let item = store.item(at: index)
        else {
            let shrink = 1 - 0.08 * (1 - present)
            return (shrink, shrink, .zero)
        }
        let fitted = ZoomableImage.fittedSize(for: item.aspectRatio, in: size)
        let targetX = frame.width / max(fitted.width, 1)
        let targetY = frame.height / max(fitted.height, 1)
        let scaleX = targetX + (1 - targetX) * present
        let scaleY = targetY + (1 - targetY) * present
        let dx = (frame.midX - (origin.x + size.width / 2)) * (1 - present)
        let dy = (frame.midY - (origin.y + size.height / 2)) * (1 - present)
        return (scaleX, scaleY, CGSize(width: dx, height: dy))
    }

    /// 关闭：hero 收向源格子 + 遮罩淡出。
    /// 用无过冲的缓出（弹簧会在末尾回弹，落点对不上格子时会「顿一下再跳」），
    /// 动画时长结束后再清空 session——缩略图恢复必须晚于最后一帧，否则会闪一下。
    /// releaseSpeed：松手时的竖向预测位移（pt）。折算成弹簧初速度，
    /// 让回场动画延续手指的速度，而不是从 0 起步（这是「丝滑」的关键）
    private func close(releaseSpeed: CGFloat = 0) {
        // 先把缩放层归零（无动画）：hero 只做「全屏 → 源格子」这一层变换，
        // 否则内层缩放/平移会与外层 hero 叠乘，落点对不上。
        zoom.reset()
        zoomed = false
        // 当前图的格子不在可视区：先让网格滚过去，等布局把帧表填好再开始 hero
        let key = store.item(at: index)?.key
        let needsScroll = key.map { coordinator.cellFrames[$0] == nil } ?? false
        if needsScroll, let key { coordinator.requestCenterScroll(key) }
        Task { @MainActor in
            if needsScroll { try? await Task.sleep(nanoseconds: 250_000_000) }
            let velocity = max(
                -2.5, min(2.5, (releaseSpeed - dismissTranslation) / Self.dismissDistance))
            withAnimation(
                .interpolatingSpring(
                    mass: 1, stiffness: 260, damping: 26, initialVelocity: velocity)
            ) {
                present = 0
                dismissTranslation = 0
            } completion: {
                coordinator.close()
            }
        }
    }

    // MARK: - 轮播

    /// 三段式画面：上一张 / 当前 / 下一张。每页都是独立的缩放视图，
    /// 只有当前页会把缩放状态回传给外层（用于让出手势）。
    private func carousel(size: CGSize) -> some View {
        let slots = [index - 1, index, index + 1].filter(items.indices.contains)
        return ZStack(alignment: .leading) {
            ForEach(slots, id: \.self) { slot in
                pageView(at: slot, size: size)
                    .frame(width: size.width, height: size.height)
                    .offset(
                        x: CGFloat(slot - index) * (size.width + Self.pageGap) + pageDrag)
            }
        }
        .frame(width: size.width, height: size.height, alignment: .leading)
        .clipped()
    }

    private func pageView(at slot: Int, size: CGSize) -> some View {
        let item = items[slot]
        let isCurrent = slot == index
        return ZoomableImage(
            url: context.api.fileURL(id: item.id),
            aspectRatio: item.aspectRatio,
            zoomController: isCurrent ? zoom : nil,
            onZoomChanged: { value in
                guard isCurrent else { return }
                zoomed = value
                // 单指拖动中途加入第二根手指时外层 drag 可能已经走了一段：放大后归零，避免页面/下拉停在半路
                if value {
                    pageDrag = 0
                    dismissTranslation = 0
                    dragMode = .idle
                }
            },
            onSingleTap: toggleChrome
        )
        .frame(width: size.width, height: size.height)
        .contentShape(Rectangle())
    }

    // MARK: - 拖动

    private func dragGesture(size: CGSize) -> some Gesture {
        DragGesture(minimumDistance: 6)
            .onChanged { value in
                guard !pageAnimating, !zoomed else { return }
                if dragMode == .idle {
                    dragMode = mode(for: value.translation)
                }
                switch dragMode {
                case .page: updatePageDrag(value.translation.width, size: size)
                case .dismiss: dismissTranslation = max(0, value.translation.height)
                case .idle: break
                }
            }
            .onEnded { value in
                guard !pageAnimating, !zoomed else {
                    dragMode = .idle
                    return
                }
                switch dragMode {
                case .page: endPageDrag(value, size: size)
                case .dismiss: endDismiss(value)
                case .idle: break
                }
                dragMode = .idle
            }
    }

    /// 主轴在拖动开始时锁定：水平翻页、竖直下拉关闭（放大时外层手势已经让出）
    private func mode(for translation: CGSize) -> DragMode {
        let horizontal = abs(translation.width) > abs(translation.height)
        return horizontal ? .page : .dismiss
    }

    // MARK: 轮播位移

    private func updatePageDrag(_ dx: CGFloat, size: CGSize) {
        if !canPage(towards: dx) {
            // 首尾页：橡皮筋抵抗，拖得越远越沉
            pageDrag = Self.rubberBand(dx, dimension: size.width)
        } else {
            pageDrag = dx
        }
    }

    private func endPageDrag(_ value: DragGesture.Value, size: CGSize) {
        let predicted = value.predictedEndTranslation.width
        if let direction = pageDirection(drag: pageDrag, predicted: predicted, size: size) {
            commitPageChange(by: direction, size: size, predicted: predicted)
        } else {
            withAnimation(Self.settleAnimation(from: pageDrag, to: 0, predicted: predicted)) {
                pageDrag = 0
            }
        }
    }

    private func endDismiss(_ value: DragGesture.Value) {
        let predicted = value.predictedEndTranslation.height
        if dismissTranslation > Self.dismissDistance || predicted > 280 {
            // 背景此刻已几乎透明：hero 收向源格子，等于相册里松手回位
            close()
        } else {
            withAnimation(.spring(response: 0.3, dampingFraction: 0.82)) { dismissTranslation = 0 }
        }
    }

    // MARK: - 翻页

    /// 当前拖动方向是否还有相邻页可翻
    private func canPage(towards offset: CGFloat) -> Bool {
        offset < 0 ? index < items.count - 1 : index > 0
    }

    /// 位移或甩动是否够翻页；返回方向（1 = 下一张，-1 = 上一张）
    private func pageDirection(
        drag: CGFloat, predicted: CGFloat, size: CGSize, ratio: CGFloat = pageDistanceRatio
    ) -> Int? {
        let threshold = size.width * ratio
        let flick = size.width * Self.pageFlickRatio
        if drag < -threshold || predicted < -flick, index < items.count - 1, drag < 0 {
            return 1
        }
        if drag > threshold || predicted > flick, index > 0, drag > 0 {
            return -1
        }
        return nil
    }

    /// 完成翻页：轮播动画滑到位后切页并归零缩放（新页从原始比例开始）
    private func commitPageChange(by direction: Int, size: CGSize, predicted: CGFloat) {
        let target = index + direction
        guard items.indices.contains(target) else { return }
        pageAnimating = true
        let destination = -CGFloat(direction) * (size.width + Self.pageGap)
        withAnimation(Self.settleAnimation(from: pageDrag, to: destination, predicted: predicted)) {
            pageDrag = destination
        } completion: {
            // 翻页前把旧页的缩放归零：它马上变成相邻页，不能带着残留的缩放/平移
            zoom.reset()
            zoomed = false
            index = target
            pageDrag = 0
            pageAnimating = false
            prefetchNeighbours(target)
        }
    }

    /// 松手瞬间的速度折算成「占剩余距离的比例」，交给弹簧作为初速度——
    /// 否则快速甩动会先被动画从 0 加速，手感发闷。夹紧到 [-3, 3] 防止极端甩动过冲。
    private static func settleAnimation(
        from current: CGFloat, to target: CGFloat, predicted: CGFloat
    )
        -> Animation
    {
        let remaining = target - current
        let velocity =
            abs(remaining) > 1
            ? max(-3, min(3, (predicted - current) / remaining)) : 0
        return .interpolatingSpring(mass: 1, stiffness: 320, damping: 30, initialVelocity: velocity)
    }

    /// UIScrollView 式橡皮筋：越拖越沉，渐近到 dimension 的一半
    private static func rubberBand(_ offset: CGFloat, dimension: CGFloat) -> CGFloat {
        let sign: CGFloat = offset < 0 ? -1 : 1
        let limit = dimension * 0.55
        guard limit > 0 else { return 0 }
        let distance = abs(offset)
        return sign * (1 - 1 / (distance / limit + 1)) * limit
    }

    // MARK: - 顶部 / 底部

    private var topBar: some View {
        HStack(alignment: .center, spacing: 12) {
            Button {
                close()
            } label: {
                Image(systemName: "xmark")
                    .font(.body.weight(.semibold))
                    .frame(width: 36, height: 36)
            }
            .buttonStyle(.glass)
            .buttonBorderShape(.circle)
            .accessibilityLabel("关闭预览")

            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(.subheadline.weight(.semibold))
                    .lineLimit(1)
                Text(subtitle)
                    .font(.caption)
                    .foregroundStyle(.white.opacity(0.72))
                    .lineLimit(1)
            }
            Spacer(minLength: 0)
        }
        .foregroundStyle(.white)
        .padding(.horizontal, HawkTheme.screenPadding)
        .padding(.top, 8)
        .padding(.bottom, 56)
        .background(
            LinearGradient(
                colors: [.black.opacity(0.8), .black.opacity(0.55), .black.opacity(0)],
                startPoint: .top, endPoint: .bottom
            )
            .ignoresSafeArea(edges: .top)
        )
    }

    private var subtitle: String {
        var parts: [String] = []
        if !items.isEmpty { parts.append("\(index + 1) / \(items.count)") }
        if let item = currentItem {
            parts.append(Format.dimensions(width: item.width, height: item.height))
            parts.append(Format.bytes(item.size))
        }
        if let annotation = detail?.annotation, !annotation.isEmpty {
            parts.append(annotation.replacingOccurrences(of: "\n", with: " "))
        }
        return parts.joined(separator: " · ")
    }

    private var bottomBar: some View {
        HStack(spacing: 26) {
            toolButton(
                title: "保存",
                systemImage: exporter.busyItemID == currentItem?.id
                    ? "arrow.down.circle" : "square.and.arrow.down",
                busy: exporter.busyItemID == currentItem?.id
            ) {
                guard let item = currentItem else { return }
                Task { await exporter.save(item, api: context.api) }
            }
            toolButton(title: "分享", systemImage: "square.and.arrow.up") {
                guard let item = currentItem else { return }
                Task {
                    await exporter.share(
                        item,
                        name: ItemKey.displayName(item: item, detail: detail),
                        api: context.api)
                }
            }
            toolButton(title: "信息", systemImage: "info.circle") {
                showsInfo = true
            }
        }
        .padding(.vertical, 10)
        .padding(.horizontal, 22)
        .glassEffect(.regular, in: Capsule())
        .padding(.bottom, 18)
    }

    private func toolButton(
        title: String, systemImage: String, busy: Bool = false, action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            VStack(spacing: 4) {
                if busy {
                    ProgressView().controlSize(.small).tint(.white)
                } else {
                    Image(systemName: systemImage).font(.system(size: 19, weight: .regular))
                }
                Text(title).font(.caption2)
            }
            .frame(minWidth: 52, minHeight: 44)
            .foregroundStyle(.white)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(busy)
        .accessibilityLabel(title)
    }

    private func toggleChrome() {
        withAnimation(.easeInOut(duration: 0.2)) { showsChrome.toggle() }
    }

    private func loadDetail(for index: Int) async {
        detail = await store.detail(at: index)
    }

    private func prefetchNeighbours(_ value: Int) {
        let neighbours = [value - 1, value + 1].compactMap { store.item(at: $0) }
        let urls = neighbours.compactMap { context.api.fileURL(id: $0.id) }
        ImageStore.shared.prefetchOriginals(urls)
    }
}

// MARK: - 素材信息

struct ItemInfoSheet: View {
    @Environment(\.dismiss) private var dismiss
    let item: Item?
    let fallback: SkeletonItem?
    let context: LibraryContext

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    if let item {
                        header(item)
                        if !item.tags.isEmpty {
                            chipsSection(title: "标签", values: item.tags, tint: HawkTheme.accent)
                        }
                        if !item.categories.isEmpty {
                            chipsSection(
                                title: "分类", values: item.categories, tint: Color(hex: "#8a6bff"))
                        }
                        if !item.folders.isEmpty {
                            chipsSection(
                                title: "文件夹", values: item.folders.map(ItemKey.displayPath),
                                tint: HawkTheme.fg1)
                        }
                        if !item.palette.isEmpty {
                            paletteSection(item.palette)
                        }
                        if let annotation = item.annotation, !annotation.isEmpty {
                            textSection(title: "备注", text: annotation)
                        }
                        if let urlString = item.url, let url = URL(string: urlString) {
                            VStack(alignment: .leading, spacing: 6) {
                                SectionCaption(text: "来源")
                                Link(urlString, destination: url)
                                    .font(.footnote)
                                    .lineLimit(2)
                            }
                        }
                        if !item.paths.isEmpty {
                            VStack(alignment: .leading, spacing: 6) {
                                SectionCaption(text: "位置")
                                ForEach(item.paths, id: \.self) { path in
                                    Text(ItemKey.displayPath(path))
                                        .font(.caption.monospaced())
                                        .foregroundStyle(HawkTheme.fg1)
                                        .textSelection(.enabled)
                                }
                            }
                        }
                    } else if let fallback {
                        VStack(alignment: .leading, spacing: 8) {
                            Text(ItemKey.displayName(item: fallback, detail: nil))
                                .font(.headline)
                            Text(
                                "\(Format.dimensions(width: fallback.width, height: fallback.height)) · \(Format.bytes(fallback.size))"
                            )
                            .font(.footnote)
                            .foregroundStyle(HawkTheme.fg1)
                            Text("正在读取完整信息…")
                                .font(.caption)
                                .foregroundStyle(HawkTheme.fg1)
                        }
                        ProgressView()
                    }
                }
                .padding(HawkTheme.screenPadding)
            }
            .background(HawkTheme.bg0)
            .navigationTitle("素材信息")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("完成") { dismiss() }
                }
            }
        }
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
    }

    private func header(_ item: Item) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(item.displayName)
                .font(.headline)
                .foregroundStyle(HawkTheme.fg0)
                .textSelection(.enabled)
            HStack(spacing: 8) {
                pill(Format.dimensions(width: item.width, height: item.height))
                pill(Format.bytes(item.size))
                if item.star > 0 {
                    pill(String(repeating: "★", count: item.star), color: HawkTheme.star)
                }
            }
            Text(Format.date(milliseconds: item.modificationTime))
                .font(.caption)
                .foregroundStyle(HawkTheme.fg1)
        }
    }

    private func pill(_ text: String, color: Color = HawkTheme.fg1) -> some View {
        Text(text)
            .font(.caption)
            .foregroundStyle(color)
            .padding(.horizontal, 8)
            .padding(.vertical, 3)
            .background(HawkTheme.bg2, in: Capsule())
    }

    private func textSection(title: String, text: String) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            SectionCaption(text: title)
            Text(text)
                .font(.subheadline)
                .foregroundStyle(HawkTheme.fg0)
                .fixedSize(horizontal: false, vertical: true)
                .textSelection(.enabled)
        }
    }

    private func chipsSection(title: String, values: [String], tint: Color) -> some View {
        VStack(alignment: .leading, spacing: 7) {
            SectionCaption(text: title)
            FlowChips(values: values, tint: tint)
        }
    }

    private func paletteSection(_ palette: [PaletteColor]) -> some View {
        VStack(alignment: .leading, spacing: 7) {
            SectionCaption(text: "调色板")
            HStack(spacing: 8) {
                ForEach(Array(palette.prefix(10).enumerated()), id: \.offset) { _, entry in
                    Circle()
                        .fill(Color(hex: entry.color))
                        .frame(width: 24, height: 24)
                        .overlay(Circle().stroke(.white.opacity(0.16), lineWidth: 1))
                }
            }
        }
    }
}

private struct FlowChips: View {
    let values: [String]
    let tint: Color

    var body: some View {
        LazyVGrid(
            columns: [GridItem(.adaptive(minimum: 56), alignment: .leading)], alignment: .leading,
            spacing: 6
        ) {
            ForEach(values, id: \.self) { value in
                Text(value)
                    .font(.caption)
                    .foregroundStyle(tint)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 4)
                    .background(tint.opacity(0.12), in: Capsule())
                    .lineLimit(1)
            }
        }
    }
}
