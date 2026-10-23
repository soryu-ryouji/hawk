import Foundation
import Security

/// 连接凭证全部放 Keychain（与设计文档「全部凭证存 Keychain」一致）。
nonisolated enum KeychainStore {
    private static let service = "com.soryu-ryouji.hawk.connections"
    private static let account = "bookmark.v1"

    static func loadBookmark() -> ConnectionBookmark? {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        guard status == errSecSuccess, let data = result as? Data else {
            return nil
        }
        return try? JSONDecoder().decode(ConnectionBookmark.self, from: data)
    }

    static func saveBookmark(_ bookmark: ConnectionBookmark) {
        guard let data = try? JSONEncoder().encode(bookmark) else {
            return
        }
        var base: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
        let updateStatus = SecItemUpdate(
            base as CFDictionary,
            [kSecValueData as String: data] as CFDictionary
        )
        if updateStatus == errSecItemNotFound {
            base[kSecValueData as String] = data
            base[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlock
            SecItemAdd(base as CFDictionary, nil)
        }
    }
}
