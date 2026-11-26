import SwiftUI

struct ProfileFormView: View {
    @Environment(\.dismiss) private var dismiss
    let kind: ConnectionKind
    let existing: ConnectionProfile?
    let onSave: (ConnectionProfile) -> Void

    @State private var name: String
    @State private var url: String
    @State private var accessKey: String
    @State private var targetIdentity: String
    @State private var targetDeviceId: String
    @State private var error: String?

    init(
        kind: ConnectionKind, existing: ConnectionProfile?,
        onSave: @escaping (ConnectionProfile) -> Void
    ) {
        self.kind = kind
        self.existing = existing
        self.onSave = onSave
        _name = State(initialValue: existing?.name ?? "")
        _url = State(initialValue: existing?.url ?? "")
        _accessKey = State(initialValue: existing?.accessKey ?? "")
        _targetIdentity = State(initialValue: existing?.targetIdentity ?? "")
        _targetDeviceId = State(initialValue: existing?.targetDeviceId ?? "")
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField(
                        kind == .lan ? "192.168.1.5:27372" : "https://hawk.example.com", text: $url
                    )
                    .keyboardType(.URL)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    TextField("显示名称（可选）", text: $name)
                } header: {
                    Text("服务地址")
                } footer: {
                    if kind == .lan {
                        Text("填写桌面端 LAN Web 地址与 AccessKey；原生加密配对请直接扫码。")
                    } else {
                        Text("hawk-server 地址、目标身份与 AccessKey 均来自桌面端配对信息。")
                    }
                }

                Section("访问凭证") {
                    SecureField("AccessKey（hk_…）", text: $accessKey)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                    if kind == .remote {
                        SecureField("target_identity（目标 iroh 公钥）", text: $targetIdentity)
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                        SecureField("target_device_id（目标设备 ID）", text: $targetDeviceId)
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                    }
                }

                if let error {
                    Section {
                        Text(error)
                            .font(.system(size: 12))
                            .foregroundStyle(HawkTheme.danger)
                    }
                }
            }
            .scrollContentBackground(.hidden)
            .background(HawkTheme.bg0)
            .navigationTitle(existing == nil ? "添加\(kind.title)连接" : "编辑连接")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("取消") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("保存并连接", action: save)
                        .disabled(!canSave)
                }
            }
        }
    }

    private var canSave: Bool {
        guard !url.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
            !accessKey.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        else { return false }
        if kind == .remote {
            return !targetIdentity.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                && !targetDeviceId.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        }
        return true
    }

    private func save() {
        var profile = existing ?? ConnectionProfile(kind: kind, url: "")
        profile.kind = kind
        profile.name = name.trimmingCharacters(in: .whitespacesAndNewlines)
        profile.url = url.trimmingCharacters(in: .whitespacesAndNewlines)
        profile.accessKey = accessKey.trimmingCharacters(in: .whitespacesAndNewlines)
        profile.targetIdentity = targetIdentity.trimmingCharacters(in: .whitespacesAndNewlines)
        profile.targetDeviceId = targetDeviceId.trimmingCharacters(in: .whitespacesAndNewlines)
        if var components = URLComponents(string: profile.url) {
            components.query = nil
            components.fragment = nil
            profile.url = components.string ?? profile.url
        }
        do {
            _ = try profile.baseURL()
        } catch {
            self.error = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
            return
        }
        onSave(profile)
        dismiss()
    }
}
