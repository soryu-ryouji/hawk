import SwiftUI

struct ProfileFormView: View {
    @Environment(\.dismiss) private var dismiss
    let existing: ConnectionProfile?
    let onSave: (ConnectionProfile) -> Void

    @State private var name: String
    @State private var url: String
    @State private var accessKey: String
    @State private var error: String?

    init(
        existing: ConnectionProfile?,
        onSave: @escaping (ConnectionProfile) -> Void
    ) {
        self.existing = existing
        self.onSave = onSave
        _name = State(initialValue: existing?.name ?? "")
        _url = State(initialValue: existing?.url ?? "")
        _accessKey = State(initialValue: existing?.accessKey ?? "")
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("192.168.1.5:27375", text: $url)
                        .keyboardType(.URL)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                    TextField("显示名称（可选）", text: $name)
                } header: {
                    Text("局域网地址")
                } footer: {
                    Text("填写桌面端 hawk-app 的局域网直连地址（默认端口 27375）与 AccessKey。")
                }

                Section("访问凭证") {
                    SecureField("AccessKey（hk_…）", text: $accessKey)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
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
            .navigationTitle(existing == nil ? "添加局域网连接" : "编辑连接")
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
        !url.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && !accessKey.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    private func save() {
        var profile = existing ?? ConnectionProfile(kind: .lan, url: "")
        profile.kind = .lan
        profile.name = name.trimmingCharacters(in: .whitespacesAndNewlines)
        profile.url = url.trimmingCharacters(in: .whitespacesAndNewlines)
        profile.accessKey = accessKey.trimmingCharacters(in: .whitespacesAndNewlines)
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
