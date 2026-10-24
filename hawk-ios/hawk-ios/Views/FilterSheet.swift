import SwiftUI

struct FilterSheet: View {
    @Environment(\.dismiss) private var dismiss
    let store: GridStore

    @State private var star: Int?
    @State private var minSide: String
    @State private var maxSide: String
    @State private var color: String?
    @State private var sizeError: String?

    private static let tiers = [480, 720, 1080, 1440, 2160, 4320]

    init(store: GridStore) {
        self.store = store
        _star = State(initialValue: store.query.star)
        _color = State(initialValue: store.query.color)
        if case .side(let min, let max) = store.query.size {
            _minSide = State(initialValue: min.map(String.init) ?? "")
            _maxSide = State(initialValue: max.map(String.init) ?? "")
        } else {
            _minSide = State(initialValue: "")
            _maxSide = State(initialValue: "")
        }
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    ScrollView(.horizontal) {
                        HStack(spacing: 8) {
                            StarChip(label: "全部", selected: star == nil) { star = nil }
                            StarChip(label: "未评分", selected: star == 0) { star = 0 }
                            ForEach(1...5, id: \.self) { value in
                                StarChip(
                                    label: String(repeating: "★", count: value),
                                    selected: star == value
                                ) {
                                    star = value
                                }
                            }
                        }
                        .padding(.vertical, 2)
                    }
                    .scrollIndicators(.hidden)
                } header: {
                    Text("评分")
                }

                Section {
                    HStack(spacing: 10) {
                        SizeField(placeholder: "最小", text: $minSide)
                        Text("–").foregroundStyle(HawkTheme.fg1)
                        SizeField(placeholder: "最大", text: $maxSide)
                    }
                    VStack(alignment: .leading, spacing: 8) {
                        Text("下限").font(.footnote).foregroundStyle(HawkTheme.fg1)
                        tierRow { minSide = String($0) }
                        Text("上限").font(.footnote).foregroundStyle(HawkTheme.fg1)
                        tierRow { maxSide = String($0) }
                    }
                    .padding(.vertical, 2)
                    if let sizeError {
                        Text(sizeError).font(.footnote).foregroundStyle(HawkTheme.danger)
                    }
                } header: {
                    Text("尺寸（短边像素）")
                } footer: {
                    Text("横图与竖图都按短边比较；留空表示不限。")
                }

                Section {
                    LazyVGrid(columns: [GridItem(.adaptive(minimum: 52), spacing: 14)], spacing: 14)
                    {
                        ForEach(HawkTheme.filterColors, id: \.self) { hex in
                            Button {
                                color = color == hex ? nil : hex
                            } label: {
                                Circle()
                                    .fill(Color(hex: hex))
                                    .frame(width: 38, height: 38)
                                    .overlay(
                                        Circle().stroke(
                                            color == hex ? HawkTheme.accent : .white.opacity(0.16),
                                            lineWidth: color == hex ? 3 : 1)
                                    )
                            }
                            .buttonStyle(.plain)
                            .accessibilityLabel("颜色 \(hex)")
                        }
                    }
                    .padding(.vertical, 4)
                    if color != nil {
                        Button("清除颜色筛选") { color = nil }
                            .font(.footnote)
                            .foregroundStyle(HawkTheme.danger)
                    }
                } header: {
                    Text("颜色")
                } footer: {
                    Text("按素材调色板近似匹配，与桌面端颜色检索一致。")
                }

                Section {
                    Button("重置全部筛选", role: .destructive) {
                        star = nil
                        minSide = ""
                        maxSide = ""
                        color = nil
                        sizeError = nil
                        store.applyFilters(star: nil, color: nil, size: nil)
                        dismiss()
                    }
                }
            }
            .scrollContentBackground(.hidden)
            .background(HawkTheme.bg0)
            .navigationTitle("筛选")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("取消") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("应用", action: apply).fontWeight(.semibold)
                }
            }
        }
    }

    private func tierRow(fill: @escaping (Int) -> Void) -> some View {
        ScrollView(.horizontal) {
            HStack(spacing: 6) {
                ForEach(Self.tiers, id: \.self) { value in
                    Button {
                        fill(value)
                    } label: {
                        Text(verbatim: "\(value)p")
                            .font(.footnote)
                            .padding(.horizontal, 9)
                            .padding(.vertical, 5)
                            .background(HawkTheme.bg3, in: Capsule())
                    }
                    .buttonStyle(.plain)
                }
            }
        }
        .scrollIndicators(.hidden)
    }

    private func apply() {
        sizeError = nil
        let minValue = parse(minSide)
        let maxValue = parse(maxSide)
        if sizeError != nil { return }
        if let minValue, let maxValue, minValue > maxValue {
            sizeError = "最小短边不能大于最大短边"
            return
        }
        let size: SizeFilter? =
            (minValue == nil && maxValue == nil) ? nil : .side(min: minValue, max: maxValue)
        store.applyFilters(star: star, color: color, size: size)
        dismiss()
    }

    private func parse(_ text: String) -> Int? {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return nil }
        guard let value = Int(trimmed), value > 0, value <= 99_999 else {
            sizeError = "尺寸需要填写 1–99999 的整数像素"
            return nil
        }
        return value
    }
}

private struct StarChip: View {
    let label: String
    let selected: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Text(label)
                .font(.subheadline.weight(selected ? .semibold : .regular))
                .foregroundStyle(selected ? HawkTheme.accent : HawkTheme.fg1)
                .padding(.horizontal, 11)
                .padding(.vertical, 6)
                .background(
                    selected ? HawkTheme.accent.opacity(0.15) : HawkTheme.bg3, in: Capsule()
                )
                .overlay(
                    Capsule().stroke(
                        selected ? HawkTheme.accent.opacity(0.6) : .clear, lineWidth: 1)
                )
        }
        .buttonStyle(.plain)
    }
}

private struct SizeField: View {
    let placeholder: String
    @Binding var text: String

    var body: some View {
        TextField(placeholder, text: $text)
            .keyboardType(.numberPad)
            .textFieldStyle(.plain)
            .foregroundStyle(HawkTheme.fg0)
            .multilineTextAlignment(.center)
            .padding(.vertical, 9)
            .background(HawkTheme.bg2, in: RoundedRectangle(cornerRadius: 9))
            .overlay(
                RoundedRectangle(cornerRadius: 9).stroke(HawkTheme.border, lineWidth: 1)
            )
    }
}
