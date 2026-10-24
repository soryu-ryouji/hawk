import SwiftUI
import UIKit

/// 全屏预览：横向分页（原生 paging scroll）、双击/双指缩放、放大后拖动、
/// 未放大时下拉关闭；顶部标题与底部玻璃工具条（保存 / 分享 / 信息）。
struct PreviewView: View {
    @Environment(\.dismiss) private var dismiss
    let context: LibraryContext
    let store: GridStore
    @State private var currentIndex: Int?
    @State private var index: Int
    @State private var zoomed = false
    @State private var dismissTranslation: CGFloat = 0
    @State private var showsChrome = true
    @State private var showsInfo: Bool
    @State private var detail: Item?
    @State private var exporter = MediaExporter()

    init(context: LibraryContext, store: GridStore, startIndex: Int, showsInfo: Bool = false) {
        self.context = context
        self.store = store
        _index = State(initialValue: startIndex)
        _currentIndex = State(initialValue: startIndex)
        _showsInfo = State(initialValue: showsInfo)
    }

    private var items: [SkeletonItem] { store.displayItems }
    private var currentItem: SkeletonItem? { store.item(at: index) }
    private var title: String {
        guard let item = currentItem else { return "" }
        return ItemKey.displayName(item: item, detail: detail)
    }

    var body: some View {
        GeometryReader { proxy in
            ZStack {
                Color.black
                    .opacity(1 - Double(min(dismissTranslation / 900, 0.4)))
                    .ignoresSafeArea()

                pager(size: proxy.size)
                    .offset(y: dismissTranslation)
                    .scaleEffect(1 - min(dismissTranslation / 1600, 0.12))

                if showsChrome {
                    VStack(spacing: 0) {
                        topBar
                        Spacer(minLength: 0)
                    }
                    .transition(.opacity)
                }
            }
            .overlay(alignment: .bottom) {
                if showsChrome {
                    bottomBar
                        .transition(.move(edge: .bottom).combined(with: .opacity))
                }
            }
            .contentShape(Rectangle())
            .onTapGesture {
                withAnimation(.easeInOut(duration: 0.2)) { showsChrome.toggle() }
            }
            .simultaneousGesture(dismissGesture)
        }
        .background(Color.black)
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

    // MARK: - 分页

    private func pager(size: CGSize) -> some View {
        ScrollView(.horizontal) {
            LazyHStack(spacing: 0) {
                ForEach(items.indices, id: \.self) { itemIndex in
                    PreviewPage(
                        url: context.api.fileURL(id: items[itemIndex].id),
                        aspectRatio: items[itemIndex].aspectRatio,
                        containerSize: size,
                        zoomed: $zoomed
                    )
                    .frame(width: size.width, height: size.height)
                }
            }
            .scrollTargetLayout()
        }
        .scrollTargetBehavior(.paging)
        .scrollPosition(id: $currentIndex, anchor: .center)
        .scrollDisabled(zoomed)
        .scrollIndicators(.hidden)
        .ignoresSafeArea()
        .onChange(of: currentIndex) { _, value in
            guard let value, value != index else { return }
            index = value
            prefetchNeighbours(value)
        }
        .onChange(of: index) { _, value in
            zoomed = false
            prefetchNeighbours(value)
        }
    }

    private var dismissGesture: some Gesture {
        DragGesture(minimumDistance: 16)
            .onChanged { value in
                guard !zoomed else { return }
                let vertical = value.translation.height
                guard abs(vertical) > abs(value.translation.width) else { return }
                dismissTranslation = max(0, vertical)
            }
            .onEnded { value in
                guard !zoomed else { return }
                let predicted = value.predictedEndTranslation.height
                if dismissTranslation > 130 || predicted > 280 {
                    dismiss()
                } else {
                    withAnimation(.spring(duration: 0.25)) { dismissTranslation = 0 }
                }
            }
    }

    // MARK: - 顶部 / 底部

    private var topBar: some View {
        HStack(alignment: .center, spacing: 12) {
            Button {
                dismiss()
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

    private func loadDetail(for index: Int) async {
        detail = await store.detail(at: index)
    }

    private func prefetchNeighbours(_ value: Int) {
        let neighbours = [value - 1, value + 1].compactMap { store.item(at: $0) }
        let urls = neighbours.compactMap { context.api.fileURL(id: $0.id) }
        ImageStore.shared.prefetchOriginals(urls)
    }
}

/// 单页图片：双击缩放、双指缩放、放大后拖动。缩放状态回传给父级（锁定分页滚动）。
private struct PreviewPage: View {
    let url: URL?
    let aspectRatio: Double
    let containerSize: CGSize
    @Binding var zoomed: Bool

    @State private var scale: CGFloat = 1
    @State private var baseScale: CGFloat = 1
    @State private var offset: CGSize = .zero
    @State private var baseOffset: CGSize = .zero
    @State private var viewSize: CGSize = .zero

    var body: some View {
        GeometryReader { proxy in
            let fitted = fittedSize(in: proxy.size)
            RemoteImage(url: url, maxPixelSize: 4096, contentMode: .fill)
                .frame(width: fitted.width, height: fitted.height)
                .scaleEffect(scale)
                .offset(offset)
                .frame(width: proxy.size.width, height: proxy.size.height)
                .contentShape(Rectangle())
                .onTapGesture(count: 2) { toggleZoom() }
                .gesture(magnify)
                .simultaneousGesture(pan)
        }
        .frame(width: containerSize.width, height: containerSize.height)
        .onGeometryChange(for: CGSize.self) {
            $0.size
        } action: {
            viewSize = $0
        }
        .onChange(of: scale) { _, value in
            zoomed = value > 1.01
        }
        .onDisappear { reset() }
    }

    /// `scaledToFit` 的尺寸：按原图比例塞进可用区域，宽高都确定，避免容器 proposal 不确定时被拉伸
    private func fittedSize(in size: CGSize) -> CGSize {
        guard size.width > 0, size.height > 0 else { return size }
        let aspect = aspectRatio > 0 ? CGFloat(aspectRatio) : 1
        let viewAspect = size.width / size.height
        if aspect > viewAspect {
            return CGSize(width: size.width, height: size.width / aspect)
        }
        return CGSize(width: size.height * aspect, height: size.height)
    }

    private var magnify: some Gesture {
        MagnifyGesture()
            .onChanged { value in
                scale = min(5, max(1, baseScale * value.magnification))
            }
            .onEnded { _ in
                baseScale = scale
                if scale <= 1.01 { reset(animated: true) } else { offset = clamped(offset) }
                baseOffset = offset
            }
    }

    private var pan: some Gesture {
        DragGesture(minimumDistance: 6)
            .onChanged { value in
                guard scale > 1.01 else { return }
                offset = clamped(
                    CGSize(
                        width: baseOffset.width + value.translation.width,
                        height: baseOffset.height + value.translation.height))
            }
            .onEnded { _ in
                baseOffset = offset
            }
    }

    private func toggleZoom() {
        withAnimation(.easeInOut(duration: 0.22)) {
            if scale > 1.01 {
                scale = 1
                baseScale = 1
                offset = .zero
                baseOffset = .zero
            } else {
                scale = 2.5
                baseScale = 2.5
                offset = .zero
                baseOffset = .zero
            }
        }
        Haptics.tap()
    }

    private func reset(animated: Bool = false) {
        let apply = {
            scale = 1
            baseScale = 1
            offset = .zero
            baseOffset = .zero
        }
        if animated {
            withAnimation(.easeOut(duration: 0.2), apply)
        } else {
            apply()
        }
    }

    /// 按图片实际显示尺寸（scaledToFit）限制平移，避免放大后拖出边界。
    private func clamped(_ value: CGSize) -> CGSize {
        guard scale > 1.01, viewSize.width > 0, viewSize.height > 0 else { return .zero }
        let display = displaySize
        let maxX = max(0, (display.width * scale - viewSize.width) / 2)
        let maxY = max(0, (display.height * scale - viewSize.height) / 2)
        return CGSize(
            width: min(maxX, max(-maxX, value.width)),
            height: min(maxY, max(-maxY, value.height))
        )
    }

    private var displaySize: CGSize { fittedSize(in: viewSize) }
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
