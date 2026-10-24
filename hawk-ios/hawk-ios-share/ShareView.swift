import SwiftUI

/// 分享扩展界面：目标素材库 + 待保存图片 + 分类/标签多选 + 保存进度。
struct ShareView: View {
    @Bindable var model: ShareModel
    let onCancel: () -> Void
    let onDone: () -> Void

    var body: some View {
        NavigationStack {
            content
                .navigationTitle("保存到 hawk")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) {
                        Button("取消", action: onCancel)
                    }
                    ToolbarItem(placement: .confirmationAction) {
                        if case .finished = model.phase {
                            Button("完成", action: onDone)
                        } else {
                            Button("保存") {
                                Task {
                                    _ = await model.upload()
                                }
                            }
                            .disabled(model.phase != .ready || model.images.isEmpty)
                        }
                    }
                }
        }
    }

    @ViewBuilder
    private var content: some View {
        switch model.phase {
        case .preparing:
            ProgressView("正在读取图片…")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        case .failed(let message):
            failureView(message)
        case .ready, .uploading, .finished:
            formView
        }
    }

    private var formView: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                if !model.targetLabel.isEmpty {
                    Label(model.targetLabel, systemImage: "externaldrive.connected.to.line.below")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }

                thumbnails

                if !model.categories.isEmpty {
                    section("分类", names: model.categories, selection: $model.selectedCategories)
                }
                if !model.tags.isEmpty {
                    section("标签", names: model.tags, selection: $model.selectedTags)
                }

                status

                Text("保存到素材库根目录；分类与标签都可以不选。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            .padding()
        }
    }

    private var thumbnails: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                ForEach(model.images) { image in
                    Image(uiImage: image.thumbnail)
                        .resizable()
                        .scaledToFill()
                        .frame(width: 72, height: 72)
                        .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
                }
            }
        }
        .overlay(alignment: .bottomTrailing) {
            if model.images.count > 1 {
                Text("\(model.images.count) 张")
                    .font(.caption2)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 2)
                    .background(.ultraThinMaterial, in: Capsule())
            }
        }
    }

    private func section(_ title: String, names: [String], selection: Binding<Set<String>>)
        -> some View
    {
        VStack(alignment: .leading, spacing: 8) {
            Text(title)
                .font(.subheadline.weight(.semibold))
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 8) {
                    ForEach(names, id: \.self) { name in
                        let selected = selection.wrappedValue.contains(name)
                        Button {
                            if selected {
                                selection.wrappedValue.remove(name)
                            } else {
                                selection.wrappedValue.insert(name)
                            }
                        } label: {
                            Text(name)
                                .font(.callout)
                                .padding(.horizontal, 12)
                                .padding(.vertical, 6)
                                .background(
                                    selected
                                        ? Color.accentColor : Color(.secondarySystemBackground),
                                    in: Capsule()
                                )
                                .foregroundStyle(selected ? Color.white : Color.primary)
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
        }
    }

    @ViewBuilder
    private var status: some View {
        switch model.phase {
        case .uploading(let done, let total):
            VStack(alignment: .leading, spacing: 6) {
                ProgressView(value: Double(done), total: Double(max(total, 1)))
                Text("正在保存 \(done) / \(total)…")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        case .finished(let saved, let skipped):
            VStack(alignment: .leading, spacing: 4) {
                Label(
                    saved > 0 ? "已保存 \(saved) 张" : "没有新增图片",
                    systemImage: "checkmark.circle.fill"
                )
                .foregroundStyle(.green)
                if skipped > 0 {
                    Text("\(skipped) 张已在素材库里，跳过")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
        default:
            EmptyView()
        }
    }

    private func failureView(_ message: String) -> some View {
        VStack(spacing: 14) {
            Image(systemName: "exclamationmark.triangle")
                .font(.largeTitle)
                .foregroundStyle(.orange)
            Text(message)
                .font(.callout)
                .multilineTextAlignment(.center)
            HStack(spacing: 12) {
                Button("关闭", action: onCancel)
                    .buttonStyle(.bordered)
                // 图片已就绪、只是上传失败（网络闪断/权限刚修好）时可直接重试，不用重走分享
                if !model.images.isEmpty {
                    Button("重试") {
                        Task {
                            _ = await model.upload()
                        }
                    }
                    .buttonStyle(.borderedProminent)
                }
            }
        }
        .padding()
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
