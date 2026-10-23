import SwiftUI

struct PreviewView: View {
    @Environment(\.dismiss) private var dismiss
    let store: LibraryStore

    @State private var index: Int
    @State private var detail: Item?
    @State private var showInfo = false
    @State private var saving = false
    @State private var showSaved = false
    @State private var saveError: String?

    @State private var scale: CGFloat = 1
    @State private var baseScale: CGFloat = 1
    @State private var offset: CGSize = .zero
    @State private var baseOffset: CGSize = .zero
    @State private var dragOffset: CGSize = .zero
    @State private var isDragging = false
    @State private var viewSize: CGSize = .zero

    init(store: LibraryStore, startIndex: Int) {
        self.store = store
        _index = State(initialValue: startIndex)
    }

    private var skeleton: SkeletonItem? { store.item(at: index) }
    private var total: Int { store.skeleton.count }
    private var title: String {
        if let detail { return detail.displayName }
        guard let skeleton else { return "" }
        let base = ItemKey.baseName(path: skeleton.path)
        let ext = ItemKey.ext(path: skeleton.path)
        return ext.isEmpty ? base : "\(base).\(ext)"
    }

    var body: some View {
        GeometryReader { proxy in
            ZStack {
                Color.black.ignoresSafeArea()

                imageLayer(size: proxy.size)

                VStack {
                    topBar
                    Spacer()
                }
            }
            .contentShape(Rectangle())
            .gesture(dragGesture(size: proxy.size))
            .simultaneousGesture(magnifyGesture)
            .onTapGesture(count: 2) {
                toggleZoom()
            }
            .onAppear { viewSize = proxy.size }
            .onChange(of: proxy.size) { _, size in
                viewSize = size
            }
        }
        .background(Color.black)
        .statusBarHidden()
        .task(id: index) {
            resetTransform(animated: false)
            detail = await store.detail(at: index)
        }
        .sheet(isPresented: $showInfo) {
            PreviewInfoSheet(detail: detail, skeleton: skeleton)
                .preferredColorScheme(.dark)
        }
        .alert("已保存到相册", isPresented: $showSaved) {
            Button("好") {}
        }
        .alert(
            "保存失败",
            isPresented: Binding(
                get: { saveError != nil },
                set: { if !$0 { saveError = nil } }
            )
        ) {
            Button("好") { saveError = nil }
        } message: {
            Text(saveError ?? "")
        }
    }

    private var topBar: some View {
        HStack(spacing: 12) {
            Button {
                dismiss()
            } label: {
                Image(systemName: "xmark")
                    .font(.system(size: 13, weight: .bold))
                    .frame(width: 34, height: 34)
                    .background(.black.opacity(0.35), in: Circle())
            }
            .accessibilityLabel("关闭预览")

            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(.system(size: 13, weight: .medium))
                    .lineLimit(1)
                Text(subtitle)
                    .font(.system(size: 10))
                    .foregroundStyle(HawkTheme.fg1)
                    .lineLimit(1)
            }

            Spacer(minLength: 4)

            Button(action: saveToPhotos) {
                Group {
                    if saving {
                        ProgressView()
                            .controlSize(.small)
                    } else {
                        Image(systemName: "square.and.arrow.down")
                            .font(.system(size: 13, weight: .semibold))
                    }
                }
                .frame(width: 34, height: 34)
                .background(.black.opacity(0.35), in: Circle())
            }
            .disabled(saving)
            .accessibilityLabel("保存到相册")

            Button {
                showInfo = true
            } label: {
                Image(systemName: "info.circle")
                    .font(.system(size: 13, weight: .semibold))
                    .frame(width: 34, height: 34)
                    .background(.black.opacity(0.35), in: Circle())
            }
            .accessibilityLabel("素材信息")
        }
        .buttonStyle(.plain)
        .foregroundStyle(.white)
        .padding(.horizontal, 14)
        .padding(.top, 12)
        .padding(.bottom, 44)
        .background(
            LinearGradient(
                stops: [
                    .init(color: Color.black.opacity(0.78), location: 0),
                    .init(color: Color.black.opacity(0.62), location: 0.55),
                    .init(color: Color.black.opacity(0), location: 1),
                ],
                startPoint: .top,
                endPoint: .bottom
            )
            .ignoresSafeArea(edges: .top)
        )
    }

    private var subtitle: String {
        var parts: [String] = []
        if total > 0 {
            parts.append("\(index + 1) / \(total)")
        }
        if let skeleton {
            parts.append(Format.dimensions(width: skeleton.width, height: skeleton.height))
            parts.append(Format.bytes(skeleton.size))
        }
        if scale > 1.01 {
            parts.append(String(format: "%.1f×", scale))
        }
        return parts.joined(separator: " · ")
    }

    @ViewBuilder
    private func imageLayer(size: CGSize) -> some View {
        ZStack {
            if let url = skeleton.flatMap({ store.api.fileURL(id: $0.id) }) {
                RemoteImage(url: url, maxPixelSize: 4096, contentMode: .fit)
                    .scaleEffect(scale)
                    .offset(
                        x: offset.width + dragOffset.width,
                        y: offset.height + dragOffset.height
                    )
            } else {
                ProgressView().tint(.white)
            }
        }
        .frame(width: size.width, height: size.height)
        .id(index)
        .opacity(dismissProgress(size: size))
        .transition(.opacity)
    }

    private func dismissProgress(size: CGSize) -> Double {
        guard scale <= 1.01, isDragging else { return 1 }
        let progress = min(1, abs(dragOffset.height) / (size.height * 0.5))
        return 1 - progress * 0.5
    }

    private var magnifyGesture: some Gesture {
        MagnificationGesture()
            .onChanged { value in
                scale = min(5, max(1, baseScale * value))
            }
            .onEnded { _ in
                baseScale = scale
                if scale <= 1.01 {
                    resetTransform(animated: true)
                } else {
                    offset = clamped(offset, in: viewSize)
                    baseOffset = offset
                }
            }
    }

    private func dragGesture(size: CGSize) -> some Gesture {
        DragGesture(minimumDistance: 8, coordinateSpace: .local)
            .onChanged { value in
                isDragging = true
                if scale > 1.01 {
                    let proposed = CGSize(
                        width: baseOffset.width + value.translation.width,
                        height: baseOffset.height + value.translation.height
                    )
                    offset = clamped(proposed, in: size)
                } else {
                    dragOffset = value.translation
                }
            }
            .onEnded { value in
                isDragging = false
                if scale > 1.01 {
                    baseOffset = offset
                    dragOffset = .zero
                    return
                }
                let horizontal = value.predictedEndTranslation.width
                let vertical = value.predictedEndTranslation.height
                if abs(horizontal) > abs(vertical), abs(horizontal) > 70 {
                    navigate(horizontal < 0 ? 1 : -1)
                } else if vertical > 120 {
                    dismiss()
                } else {
                    withAnimation(.easeOut(duration: 0.18)) {
                        dragOffset = .zero
                    }
                }
            }
    }

    private func navigate(_ delta: Int) {
        let next = index + delta
        guard store.skeleton.indices.contains(next) else {
            withAnimation(.easeOut(duration: 0.18)) {
                dragOffset = .zero
            }
            return
        }
        withAnimation(.easeOut(duration: 0.16)) {
            dragOffset = .zero
            index = next
        }
    }

    private func toggleZoom() {
        withAnimation(.easeInOut(duration: 0.2)) {
            if scale > 1.01 {
                scale = 1
                baseScale = 1
                offset = .zero
                baseOffset = .zero
            } else {
                scale = 2.5
                baseScale = 2.5
            }
        }
    }

    private func resetTransform(animated: Bool) {
        let apply = {
            scale = 1
            baseScale = 1
            offset = .zero
            baseOffset = .zero
            dragOffset = .zero
            isDragging = false
        }
        if animated {
            withAnimation(.easeInOut(duration: 0.2), apply)
        } else {
            apply()
        }
    }

    /// 按图片实际显示尺寸（scaledToFit）限制平移，避免放大后拖出边界。
    private func clamped(_ value: CGSize, in size: CGSize) -> CGSize {
        guard scale > 1.01, size.width > 0, size.height > 0 else { return .zero }
        let display = displaySize(in: size)
        let maxX = max(0, (display.width * scale - size.width) / 2)
        let maxY = max(0, (display.height * scale - size.height) / 2)
        return CGSize(
            width: min(maxX, max(-maxX, value.width)),
            height: min(maxY, max(-maxY, value.height))
        )
    }

    private func displaySize(in size: CGSize) -> CGSize {
        guard let skeleton, skeleton.width > 0, skeleton.height > 0 else { return size }
        let aspect = CGFloat(skeleton.width) / CGFloat(skeleton.height)
        let viewAspect = size.width / size.height
        if aspect > viewAspect {
            return CGSize(width: size.width, height: size.width / aspect)
        }
        return CGSize(width: size.height * aspect, height: size.height)
    }

    private func saveToPhotos() {
        guard let skeleton, !saving else { return }
        saving = true
        Task {
            defer { saving = false }
            do {
                let data = try await store.api.fetchFileData(id: skeleton.id)
                try await PhotoSaver.save(data)
                showSaved = true
            } catch {
                saveError =
                    (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
            }
        }
    }
}

private struct PreviewInfoSheet: View {
    @Environment(\.dismiss) private var dismiss
    let detail: Item?
    let skeleton: SkeletonItem?

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    if let detail {
                        header(detail)
                        if !detail.tags.isEmpty {
                            chipsSection(title: "标签", values: detail.tags, tint: HawkTheme.accent)
                        }
                        if !detail.categories.isEmpty {
                            chipsSection(
                                title: "分类", values: detail.categories, tint: Color(hex: "#8a6bff"))
                        }
                        if !detail.folders.isEmpty {
                            chipsSection(
                                title: "文件夹", values: detail.folders.map(ItemKey.displayPath),
                                tint: HawkTheme.fg1)
                        }
                        if !detail.palette.isEmpty {
                            paletteSection(detail.palette)
                        }
                        if let annotation = detail.annotation, !annotation.isEmpty {
                            VStack(alignment: .leading, spacing: 6) {
                                sectionTitle("备注")
                                Text(annotation)
                                    .font(.system(size: 13))
                                    .foregroundStyle(HawkTheme.fg0)
                                    .fixedSize(horizontal: false, vertical: true)
                            }
                        }
                        if let urlString = detail.url, let url = URL(string: urlString) {
                            VStack(alignment: .leading, spacing: 6) {
                                sectionTitle("来源")
                                Link(urlString, destination: url)
                                    .font(.system(size: 12))
                                    .lineLimit(1)
                            }
                        }
                        if !detail.paths.isEmpty {
                            VStack(alignment: .leading, spacing: 6) {
                                sectionTitle("位置")
                                ForEach(detail.paths, id: \.self) { path in
                                    Text(ItemKey.displayPath(path))
                                        .font(.system(size: 11, design: .monospaced))
                                        .foregroundStyle(HawkTheme.fg1)
                                        .textSelection(.enabled)
                                }
                            }
                        }
                    } else {
                        ProgressView("正在读取素材信息…")
                            .frame(maxWidth: .infinity)
                            .padding(.top, 40)
                    }
                }
                .padding(20)
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
    }

    private func header(_ item: Item) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(item.displayName)
                .font(.system(size: 16, weight: .semibold))
                .foregroundStyle(HawkTheme.fg0)
                .textSelection(.enabled)
            HStack(spacing: 10) {
                infoPill(Format.dimensions(width: item.width, height: item.height))
                infoPill(Format.bytes(item.size))
                if item.star > 0 {
                    infoPill(String(repeating: "★", count: item.star), color: HawkTheme.star)
                }
            }
            Text(Format.date(milliseconds: item.modificationTime))
                .font(.system(size: 11))
                .foregroundStyle(HawkTheme.fg1)
        }
    }

    private func infoPill(_ text: String, color: Color = HawkTheme.fg1) -> some View {
        Text(text)
            .font(.system(size: 11))
            .foregroundStyle(color)
            .padding(.horizontal, 8)
            .padding(.vertical, 3)
            .background(HawkTheme.bg2, in: Capsule())
    }

    private func chipsSection(title: String, values: [String], tint: Color) -> some View {
        VStack(alignment: .leading, spacing: 7) {
            sectionTitle(title)
            FlowChips(values: values, tint: tint)
        }
    }

    private func paletteSection(_ palette: [PaletteColor]) -> some View {
        VStack(alignment: .leading, spacing: 7) {
            sectionTitle("调色板")
            HStack(spacing: 8) {
                ForEach(Array(palette.prefix(10).enumerated()), id: \.offset) { _, entry in
                    Circle()
                        .fill(Color(hex: entry.color))
                        .frame(width: 22, height: 22)
                        .overlay(Circle().stroke(Color.white.opacity(0.16), lineWidth: 1))
                }
            }
        }
    }

    private func sectionTitle(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 11, weight: .semibold))
            .foregroundStyle(HawkTheme.fg1)
    }
}

private struct FlowChips: View {
    let values: [String]
    let tint: Color

    var body: some View {
        LazyVGrid(
            columns: [GridItem(.adaptive(minimum: 52), alignment: .leading)], alignment: .leading,
            spacing: 6
        ) {
            ForEach(values, id: \.self) { value in
                Text(value)
                    .font(.system(size: 11))
                    .foregroundStyle(tint)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 4)
                    .background(tint.opacity(0.12), in: Capsule())
                    .lineLimit(1)
            }
        }
    }
}
