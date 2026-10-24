import SwiftUI

struct SettingsView: View {
    @Environment(AppModel.self) private var app
    @Environment(\.dismiss) private var dismiss
    @AppStorage("hawk.ios.thumbSize") private var storedThumbSize = 0.0
    @Environment(\.horizontalSizeClass) private var horizontalSizeClass

    let context: LibraryContext

    private var defaultThumbSize: Double { horizontalSizeClass == .regular ? 240 : 130 }
    private var thumbSize: Double { storedThumbSize > 0 ? storedThumbSize : defaultThumbSize }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    LabeledContent("素材库", value: context.library?.name ?? context.title)
                    LabeledContent(
                        "连接方式", value: context.isRemote ? "广域网（hawk-server）" : "局域网直连")
                    LabeledContent("访问级别", value: context.accessLabel)
                    if let device = context.targetDeviceName, !device.isEmpty, context.isRemote {
                        LabeledContent("目标设备", value: device)
                    }
                    LabeledContent("服务地址", value: context.profile.url)
                    LabeledContent(
                        "素材数量",
                        value: context.countsLoading ? "…" : "\(context.counts.all) 项")
                    if let label = context.scanningLabel {
                        LabeledContent("后台任务", value: label)
                    }
                } header: {
                    Text("当前连接")
                } footer: {
                    Text("iOS 客户端为只读浏览；导入、编辑与删除请在桌面端操作。")
                }

                Section("连接操作") {
                    Button {
                        app.reconnect()
                        dismiss()
                    } label: {
                        Label("重新连接", systemImage: "arrow.clockwise")
                    }
                    Button {
                        app.showConnectScreen()
                        dismiss()
                    } label: {
                        Label("切换素材库", systemImage: "rectangle.2.swap")
                    }
                    Button(role: .destructive) {
                        app.disconnect()
                        dismiss()
                    } label: {
                        Label("断开连接", systemImage: "xmark.circle")
                    }
                }

                Section {
                    LabeledContent("缩略图尺寸", value: "\(Int(thumbSize))")
                    Slider(
                        value: Binding(
                            get: { thumbSize },
                            set: { storedThumbSize = $0 }
                        ), in: 90...300, step: 5
                    )
                    .tint(HawkTheme.accent)
                    if storedThumbSize > 0 {
                        Button("恢复默认尺寸") { storedThumbSize = 0 }
                            .font(.footnote)
                    }
                } header: {
                    Text("外观")
                } footer: {
                    Text("数值是每行目标高度：网格按图片比例齐行，越大看得越清楚，越小一屏放得越多。")
                }

                Section("关于") {
                    LabeledContent("hawk iOS", value: clientVersion)
                    if let version = context.library?.applicationVersion {
                        LabeledContent("桌面端版本", value: version)
                    }
                    if let mode = context.library?.storageMode {
                        LabeledContent("元数据存储", value: storageLabel(mode))
                    }
                    if let scan = context.library?.scan {
                        LabeledContent(
                            "周期扫描",
                            value: scan.periodic
                                ? "开启\(scan.interval.map { "（\($0) 分钟）" } ?? "")" : "关闭")
                    }
                }
            }
            .scrollContentBackground(.hidden)
            .background(HawkTheme.bg0)
            .navigationTitle("连接信息")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("完成") { dismiss() }
                }
            }
        }
    }

    private func storageLabel(_ mode: String) -> String {
        switch mode {
        case "database": "SQLite 单库（metadata.db）"
        case "toml": "TOML 文件（网盘同步友好）"
        default: mode
        }
    }

    private var clientVersion: String {
        let version = Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "1.0"
        let build = Bundle.main.infoDictionary?["CFBundleVersion"] as? String ?? "1"
        return "\(version) (\(build))"
    }
}
