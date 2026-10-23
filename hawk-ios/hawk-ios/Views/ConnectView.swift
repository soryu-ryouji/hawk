import SwiftUI

struct ConnectView: View {
    @Environment(AppModel.self) private var app
    @State private var formTarget: ProfileFormTarget?
    @State private var showScanner = false
    @State private var scanError: String?

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 24) {
                    header

                    if let error = app.lastError {
                        ErrorBanner(message: error)
                    }

                    actions

                    profileList
                }
                .padding(20)
                .padding(.bottom, 24)
            }
            .scrollIndicators(.hidden)
            .hawkScreenBackground()
            .navigationTitle("hawk")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button {
                        showScanner = true
                    } label: {
                        Image(systemName: "qrcode.viewfinder")
                    }
                    .accessibilityLabel("扫描配对二维码")
                }
            }
            .sheet(item: $formTarget) { target in
                ProfileFormView(kind: target.kind, existing: target.profile) { profile in
                    app.saveAndConnect(profile)
                }
            }
            .sheet(isPresented: $showScanner) {
                QRScannerSheet { code in
                    showScanner = false
                    handleScannedCode(code)
                }
            }
            .alert(
                "二维码无法识别",
                isPresented: Binding(
                    get: { scanError != nil },
                    set: { if !$0 { scanError = nil } }
                )
            ) {
                Button("好") { scanError = nil }
            } message: {
                Text(scanError ?? "")
            }
        }
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 14) {
                HawkMark(size: 58)
                VStack(alignment: .leading, spacing: 3) {
                    Text("hawk")
                        .font(.system(size: 24, weight: .bold))
                        .foregroundStyle(HawkTheme.fg0)
                    Text("在 iPhone / iPad 上浏览你的素材库")
                        .font(.system(size: 13))
                        .foregroundStyle(HawkTheme.fg1)
                }
            }
            Text("连接同一局域网内的桌面端，或经自托管 hawk-server 远程查看。当前客户端为只读浏览。")
                .font(.system(size: 12))
                .foregroundStyle(HawkTheme.fg1)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private var actions: some View {
        HStack(spacing: 10) {
            Button {
                formTarget = ProfileFormTarget(kind: .lan, profile: nil)
            } label: {
                Label("局域网", systemImage: "wifi")
                    .font(.system(size: 13, weight: .medium))
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 10)
            }
            .buttonStyle(.borderedProminent)

            Button {
                formTarget = ProfileFormTarget(kind: .remote, profile: nil)
            } label: {
                Label("广域网", systemImage: "globe")
                    .font(.system(size: 13, weight: .medium))
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 10)
            }
            .buttonStyle(.bordered)
        }
    }

    @ViewBuilder
    private var profileList: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("已保存的连接")
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(HawkTheme.fg1)

            if app.connections.profiles.isEmpty {
                Text(
                    "还没有连接。局域网连接需要桌面端「设置 → 局域网」里的地址和 token；广域网连接需要服务器地址、service-key 与 hawk-app key。"
                )
                .font(.system(size: 12))
                .foregroundStyle(HawkTheme.fg1)
                .padding(14)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(HawkTheme.bg1, in: RoundedRectangle(cornerRadius: 10))
            } else {
                ForEach(app.connections.profiles) { profile in
                    ProfileRow(
                        profile: profile,
                        active: app.connections.activeID == profile.id,
                        connect: { app.connect(profile) },
                        edit: {
                            formTarget = ProfileFormTarget(kind: profile.kind, profile: profile)
                        },
                        delete: { app.deleteProfile(profile) }
                    )
                }
            }
        }
    }

    private func handleScannedCode(_ code: String) {
        do {
            let profile = try PairingPayload.decode(code).profile()
            app.saveAndConnect(profile)
        } catch {
            scanError = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
    }
}

private struct ProfileFormTarget: Identifiable {
    let id = UUID()
    let kind: ConnectionKind
    let profile: ConnectionProfile?
}

private struct ProfileRow: View {
    let profile: ConnectionProfile
    let active: Bool
    let connect: () -> Void
    let edit: () -> Void
    let delete: () -> Void

    var body: some View {
        HStack(spacing: 12) {
            Button(action: connect) {
                HStack(spacing: 12) {
                    Image(systemName: profile.kind == .lan ? "wifi" : "globe")
                        .font(.system(size: 16))
                        .foregroundStyle(HawkTheme.accent)
                        .frame(width: 30, height: 30)
                        .background(HawkTheme.bg3, in: RoundedRectangle(cornerRadius: 8))
                    VStack(alignment: .leading, spacing: 3) {
                        HStack(spacing: 6) {
                            Text(profile.displayName)
                                .font(.system(size: 14, weight: .medium))
                                .foregroundStyle(HawkTheme.fg0)
                                .lineLimit(1)
                            if active {
                                Text("上次使用")
                                    .font(.system(size: 10))
                                    .foregroundStyle(HawkTheme.accent)
                                    .padding(.horizontal, 5)
                                    .padding(.vertical, 1)
                                    .background(HawkTheme.accent.opacity(0.14), in: Capsule())
                            }
                        }
                        Text("\(profile.kind.title) · \(profile.url)")
                            .font(.system(size: 11))
                            .foregroundStyle(HawkTheme.fg1)
                            .lineLimit(1)
                    }
                    Spacer(minLength: 8)
                    Image(systemName: "chevron.right")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(HawkTheme.fg1)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            Menu {
                Button("编辑", systemImage: "pencil", action: edit)
                Button("删除", systemImage: "trash", role: .destructive, action: delete)
            } label: {
                Image(systemName: "ellipsis")
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundStyle(HawkTheme.fg1)
                    .frame(width: 30, height: 30)
            }
        }
        .padding(12)
        .background(HawkTheme.bg1, in: RoundedRectangle(cornerRadius: 10))
        .overlay(
            RoundedRectangle(cornerRadius: 10)
                .stroke(active ? HawkTheme.accent.opacity(0.55) : HawkTheme.border, lineWidth: 1)
        )
    }
}

struct ErrorBanner: View {
    let message: String

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: "exclamationmark.circle.fill")
                .foregroundStyle(HawkTheme.danger)
            Text(message)
                .font(.system(size: 12))
                .foregroundStyle(HawkTheme.fg0)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
        }
        .padding(12)
        .background(HawkTheme.danger.opacity(0.12), in: RoundedRectangle(cornerRadius: 8))
        .overlay(
            RoundedRectangle(cornerRadius: 8)
                .stroke(HawkTheme.danger.opacity(0.35), lineWidth: 1)
        )
    }
}
