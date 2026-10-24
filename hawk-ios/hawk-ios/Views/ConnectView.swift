import SwiftUI
import UIKit

struct ConnectView: View {
    @Environment(AppModel.self) private var app
    @State private var formTarget: ProfileFormTarget?
    @State private var showScanner = false
    @State private var scanError: String?

    var body: some View {
        NavigationStack {
            List {
                Section {
                    hero
                        .listRowBackground(Color.clear)
                        .listRowInsets(EdgeInsets(top: 8, leading: 4, bottom: 8, trailing: 4))
                }

                if let error = app.lastError {
                    Section {
                        ErrorBanner(message: error)
                            .listRowBackground(Color.clear)
                            .listRowInsets(EdgeInsets())
                    }
                }

                Section {
                    Button {
                        showScanner = true
                    } label: {
                        Label("扫描配对二维码", systemImage: "qrcode.viewfinder")
                            .font(.body.weight(.semibold))
                            .frame(maxWidth: .infinity)
                            .padding(.vertical, 6)
                    }
                    .buttonStyle(.borderedProminent)
                    .listRowInsets(EdgeInsets(top: 6, leading: 12, bottom: 6, trailing: 12))
                    .listRowBackground(Color.clear)

                    Button {
                        pastePairingCode()
                    } label: {
                        Label("从剪贴板导入配对信息", systemImage: "doc.on.clipboard")
                    }

                    Menu {
                        Button("局域网连接", systemImage: "wifi") {
                            formTarget = ProfileFormTarget(kind: .lan, profile: nil)
                        }
                        Button("广域网连接", systemImage: "globe") {
                            formTarget = ProfileFormTarget(kind: .remote, profile: nil)
                        }
                    } label: {
                        Label("手动填写地址与凭证", systemImage: "square.and.pencil")
                    }
                } footer: {
                    Text("桌面端「设置 → 局域网」可生成配对二维码，扫码即可完成连接。")
                }

                Section("已保存的连接") {
                    if app.connections.profiles.isEmpty {
                        Text("还没有连接。")
                            .font(.footnote)
                            .foregroundStyle(HawkTheme.fg1)
                    } else {
                        ForEach(app.connections.profiles) { profile in
                            ProfileRow(
                                profile: profile,
                                active: app.connections.activeID == profile.id
                            ) {
                                app.connect(profile)
                            }
                            .swipeActions(edge: .trailing, allowsFullSwipe: false) {
                                Button(role: .destructive) {
                                    app.deleteProfile(profile)
                                } label: {
                                    Label("删除", systemImage: "trash")
                                }
                                Button {
                                    formTarget = ProfileFormTarget(
                                        kind: profile.kind, profile: profile)
                                } label: {
                                    Label("编辑", systemImage: "pencil")
                                }
                                .tint(HawkTheme.accent)
                            }
                        }
                    }
                }
            }
            .listStyle(.insetGrouped)
            .scrollContentBackground(.hidden)
            .background(HawkTheme.bg0)
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
                "无法导入配对信息",
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

    private var hero: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 14) {
                HawkMark(size: 56)
                VStack(alignment: .leading, spacing: 2) {
                    Text("hawk")
                        .font(.title.weight(.bold))
                        .foregroundStyle(HawkTheme.fg0)
                    Text("在 iPhone / iPad 上浏览你的素材库")
                        .font(.subheadline)
                        .foregroundStyle(HawkTheme.fg1)
                }
            }
            Text("连接同一局域网内的桌面端，或经自托管 hawk-server 远程查看。")
                .font(.footnote)
                .foregroundStyle(HawkTheme.fg1)
                .fixedSize(horizontal: false, vertical: true)
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

    private func pastePairingCode() {
        guard let text = UIPasteboard.general.string, !text.isEmpty else {
            scanError = "剪贴板中没有配对信息"
            return
        }
        handleScannedCode(text.trimmingCharacters(in: .whitespacesAndNewlines))
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

    var body: some View {
        Button(action: connect) {
            HStack(spacing: 12) {
                Image(systemName: profile.kind == .lan ? "wifi" : "globe")
                    .font(.body)
                    .foregroundStyle(HawkTheme.accent)
                    .frame(width: 34, height: 34)
                    .background(HawkTheme.bg3, in: RoundedRectangle(cornerRadius: 9))
                VStack(alignment: .leading, spacing: 3) {
                    HStack(spacing: 6) {
                        Text(profile.displayName)
                            .font(.body.weight(.medium))
                            .foregroundStyle(HawkTheme.fg0)
                            .lineLimit(1)
                        if active {
                            Text("上次使用")
                                .font(.caption2)
                                .foregroundStyle(HawkTheme.accent)
                                .padding(.horizontal, 6)
                                .padding(.vertical, 2)
                                .background(HawkTheme.accent.opacity(0.14), in: Capsule())
                        }
                    }
                    Text("\(profile.kind.title) · \(profile.url)")
                        .font(.caption)
                        .foregroundStyle(HawkTheme.fg1)
                        .lineLimit(1)
                }
                Spacer(minLength: 4)
                Image(systemName: "chevron.right")
                    .font(.footnote.weight(.semibold))
                    .foregroundStyle(HawkTheme.fg1)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}
