import SwiftUI

/// 锁定视图的内容区占位：输入密码原地解锁，成功后重查当前视图。
/// 非模态——侧栏与工具栏照常可用，点别处即可离开（与桌面端一致）。
struct LockScreen: View {
    let context: LibraryContext
    let target: LockTarget
    var onUnlocked: () -> Void

    @State private var password = ""
    @State private var busy = false
    @State private var error: String?
    @FocusState private var focused: Bool

    var body: some View {
        VStack(spacing: 14) {
            Image(systemName: "lock.fill")
                .font(.system(size: 42, weight: .regular))
                .foregroundStyle(HawkTheme.fg1)
                .padding(.bottom, 2)

            VStack(spacing: 6) {
                Text("此\(target.dimension.title)已锁定")
                    .font(.title3.weight(.semibold))
                    .foregroundStyle(HawkTheme.fg0)
                Text("「\(target.displayName)」的内容需要密码解锁后查看")
                    .font(.subheadline)
                    .foregroundStyle(HawkTheme.fg1)
                    .multilineTextAlignment(.center)
            }

            VStack(spacing: 10) {
                SecureField("密码", text: $password)
                    .textFieldStyle(.plain)
                    .foregroundStyle(HawkTheme.fg0)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 11)
                    .background(HawkTheme.bg2, in: RoundedRectangle(cornerRadius: 10))
                    .overlay(
                        RoundedRectangle(cornerRadius: 10)
                            .stroke(HawkTheme.border, lineWidth: 1)
                    )
                    .submitLabel(.go)
                    .focused($focused)
                    .onSubmit { Task { await unlock() } }
                    .frame(maxWidth: 320)

                if let error {
                    Text(error)
                        .font(.footnote)
                        .foregroundStyle(HawkTheme.danger)
                        .multilineTextAlignment(.center)
                        .transition(.opacity)
                }

                Button {
                    Task { await unlock() }
                } label: {
                    Group {
                        if busy {
                            ProgressView().controlSize(.small)
                        } else {
                            Text("解锁")
                        }
                    }
                    .frame(maxWidth: 320)
                }
                .buttonStyle(.borderedProminent)
                .disabled(password.isEmpty || busy)
            }
            .padding(.top, 6)

            Text("解锁仅对当前设备生效，可在浏览页重新锁定")
                .font(.caption)
                .foregroundStyle(HawkTheme.fg1)
                .padding(.top, 4)
        }
        .padding(28)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .onAppear { focused = true }
        .task { await debugAutoUnlock() }
        .animation(.easeOut(duration: 0.15), value: error)
    }

    #if DEBUG
        /// 本地冒烟：模拟器无法注入键盘输入，用环境变量自动填密码走一遍解锁链路
        private func debugAutoUnlock() async {
            guard let password = ProcessInfo.processInfo.environment["HAWK_IOS_DEBUG_UNLOCK"],
                !password.isEmpty
            else { return }
            self.password = password
            await unlock()
        }
    #else
        private func debugAutoUnlock() async {}
    #endif

    private func unlock() async {
        guard !busy, !password.isEmpty else { return }
        busy = true
        error = nil
        defer { busy = false }
        do {
            try await context.unlock(target, password: password)
            Haptics.success()
            password = ""
            focused = false
            onUnlocked()
        } catch {
            Haptics.warning()
            focused = true
            if let apiError = error as? APIError {
                self.error =
                    apiError.isThrottled ? apiError.message : "密码不正确，请重新输入"
            } else {
                self.error =
                    (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
            }
        }
    }
}
