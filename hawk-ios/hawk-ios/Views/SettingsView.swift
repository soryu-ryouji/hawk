import SwiftUI

struct SettingsView: View {
    @Environment(AppModel.self) private var app
    @Environment(\.dismiss) private var dismiss
    @AppStorage("hawk.ios.thumbSize") private var thumbSize = 200.0

    let session: LibrarySession

    private var store: LibraryStore { session.store }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    LabeledContent("素材库", value: store.library?.name ?? session.label)
                    LabeledContent("连接方式", value: session.isRemote ? "广域网（hawk-server）" : "局域网直连")
                    LabeledContent("访问级别", value: session.accessLabel)
                    if session.isRemote {
                        if let device = session.targetDeviceName, !device.isEmpty {
                            LabeledContent("目标设备", value: device)
                        }
                    }
                    LabeledContent("服务地址", value: session.profile.url)
                    LabeledContent("全部条目", value: store.countsLoading ? "…" : "\(store.counts.all)")
                    LabeledContent(
                        "当前视图", value: "\(store.total) 项 · \(Format.bytes(store.totalSize))")
                } header: {
                    Text("当前连接")
                } footer: {
                    Text("iOS 客户端当前为只读浏览；导入、编辑与删除请在桌面端操作。")
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
                        Label("切换连接", systemImage: "rectangle.2.swap")
                    }
                    Button(role: .destructive) {
                        app.disconnect()
                        dismiss()
                    } label: {
                        Label("断开连接", systemImage: "xmark.circle")
                    }
                }

                Section {
                    HStack {
                        Text("缩略图尺寸")
                        Spacer()
                        Text("\(Int(thumbSize))")
                            .foregroundStyle(HawkTheme.fg1)
                            .monospacedDigit()
                    }
                    Slider(value: $thumbSize, in: 120...280, step: 8)
                        .tint(HawkTheme.accent)
                    Text("贴近桌面端的齐行网格：数值是每行目标高度，不再是固定列数。")
                        .font(.system(size: 11))
                        .foregroundStyle(HawkTheme.fg1)
                } header: {
                    Text("外观")
                }

                Section("关于") {
                    LabeledContent("hawk iOS", value: clientVersion)
                    if let version = store.library?.applicationVersion {
                        LabeledContent("桌面端版本", value: version)
                    }
                    if let mode = store.library?.storageMode {
                        LabeledContent("元数据存储", value: mode)
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

    private var clientVersion: String {
        let version = Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "1.0"
        let build = Bundle.main.infoDictionary?["CFBundleVersion"] as? String ?? "1"
        return "\(version) (\(build))"
    }
}
