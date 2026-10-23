import SwiftUI

struct ProfileFormView: View {
    @Environment(\.dismiss) private var dismiss
    let kind: ConnectionKind
    let existing: ConnectionProfile?
    let onSave: (ConnectionProfile) -> Void

    @State private var name: String
    @State private var url: String
    @State private var token: String
    @State private var serviceKey: String
    @State private var appKey: String
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
        _token = State(initialValue: existing?.token ?? "")
        _serviceKey = State(initialValue: existing?.serviceKey ?? "")
        _appKey = State(initialValue: existing?.appKey ?? "")
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
                        Text("桌面端：设置 → 局域网 → 开启网页访问，复制监听地址与访问 token。")
                    } else {
                        Text("hawk-server 的部署地址；service-key 与 hawk-app key 来自服务端与桌面端。")
                    }
                }

                if kind == .lan {
                    Section("访问凭证") {
                        SecureField("访问 token", text: $token)
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                    }
                } else {
                    Section("访问凭证") {
                        SecureField("service-key（hs_…）", text: $serviceKey)
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                        SecureField("hawk-app key（hk_…）", text: $appKey)
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
        guard !url.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return false }
        switch kind {
        case .lan:
            return !token.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        case .remote:
            return !serviceKey.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                && !appKey.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        }
    }

    private func save() {
        var profile = existing ?? ConnectionProfile(kind: kind, url: "")
        profile.kind = kind
        profile.name = name.trimmingCharacters(in: .whitespacesAndNewlines)
        profile.url = url.trimmingCharacters(in: .whitespacesAndNewlines)
        profile.token = token.trimmingCharacters(in: .whitespacesAndNewlines)
        // 兼容直接粘贴局域网查看器地址（http://ip:port/?token=...）：token 缺省时从查询参数补。
        if kind == .lan, profile.token.isEmpty,
            let components = URLComponents(string: profile.url),
            let value = components.queryItems?.first(where: { $0.name == "token" })?.value
        {
            profile.token = value
        }
        if var components = URLComponents(string: profile.url) {
            components.query = nil
            components.fragment = nil
            profile.url = components.string ?? profile.url
        }
        profile.serviceKey = serviceKey.trimmingCharacters(in: .whitespacesAndNewlines)
        profile.appKey = appKey.trimmingCharacters(in: .whitespacesAndNewlines)
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
