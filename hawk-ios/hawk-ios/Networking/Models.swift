import Foundation

// API 契约：字段与 hawk-daemon 的 server-rest-api-v1.md 一一对应。
// 解码统一走 convertFromSnakeCase，编码统一走 convertToSnakeCase。

nonisolated struct AppInfo: Decodable, Sendable {
    let version: String
    let platform: String?
    let access: String
    let hasLibrary: Bool
    let writable: Bool
    let lan: LanInfo?
}

nonisolated struct LanInfo: Decodable, Sendable {
    let active: Bool
    let port: Int?
    let error: String?
}

nonisolated struct StartupInfo: Decodable, Sendable {
    let status: String
    let phase: String?
    let processed: Int?
    let total: Int?
    let message: String?
}

nonisolated struct LibraryInfo: Decodable, Sendable {
    let name: String
    let path: String?
    let applicationVersion: String?
    let storageMode: String?
    let modificationTime: Int64?
}

nonisolated struct FolderNode: Decodable, Sendable, Identifiable {
    let path: String
    let name: String
    let count: Int
    let modificationTime: Int64?
    let children: [FolderNode]

    var id: String { path }
}

nonisolated struct TaxonInfo: Decodable, Sendable, Identifiable {
    let name: String
    let count: Int

    var id: String { name }
}

nonisolated struct GlobalFilterSnapshot: Decodable, Sendable {
    let folders: [String]
    let categories: [String]
    let tags: [String]

    static let empty = GlobalFilterSnapshot(folders: [], categories: [], tags: [])
}

nonisolated struct SkeletonItem: Decodable, Sendable, Hashable {
    let id: String
    let path: String
    let width: Int
    let height: Int
    let star: Int
    let size: Int64

    var key: String { "\(id)\n\(path)" }
    var aspectRatio: Double { width > 0 && height > 0 ? Double(width) / Double(height) : 1 }
}

nonisolated struct Item: Decodable, Sendable {
    let id: String
    let name: String
    let ext: String
    let width: Int
    let height: Int
    let size: Int64
    let url: String?
    let tags: [String]
    let categories: [String]
    let paths: [String]
    let folders: [String]
    let star: Int
    let annotation: String?
    let modificationTime: Int64
    let palette: [PaletteColor]

    var path: String { paths.first ?? "" }
    var key: String { "\(id)\n\(path)" }
    var displayName: String { ext.isEmpty ? name : "\(name).\(ext)" }
}

nonisolated struct PaletteColor: Decodable, Sendable {
    let color: String
    let percentage: Double
}

nonisolated struct ItemListResponse: Decodable, Sendable {
    let items: [Item]
    let total: Int
    let totalSize: Int64
    let offset: Int
    let limit: Int
}

nonisolated struct ItemSkeletonResponse: Decodable, Sendable {
    let items: [SkeletonItem]
    let totalSize: Int64
}

nonisolated struct GateResponse: Decodable, Sendable {
    let gateToken: String
    let expiresIn: Int
}

nonisolated struct LoginResponse: Decodable, Sendable {
    let token: String
    let access: String
    let deviceId: String
}

nonisolated struct ViewerSession: Decodable, Sendable {
    let sessionToken: String
    let access: String
    let targetDeviceId: String
    let targetName: String
    let targetLibraryName: String
}

// MARK: - Requests

nonisolated struct ItemListRequest: Encodable, Sendable {
    var ids: [String]?
    var keywords: [String]?
    var tags: [String]?
    var categories: [String]?
    var star: Int?
    var folders: [String]?
    var foldersExact: Bool?
    var withoutCategories: Bool?
    var withoutTags: Bool?
    var excludeFolders: [String]?
    var excludeCategories: [String]?
    var excludeTags: [String]?
    var color: String?
    var minSideGte: Int?
    var minSideLte: Int?
    var inTrash: Bool?
    var orderBy: String
    var order: String
    var offset: Int?
    var limit: Int?

    init(orderBy: String = "modification_time", order: String = "desc") {
        self.orderBy = orderBy
        self.order = order
    }
}

nonisolated struct GateRequest: Encodable, Sendable {
    let serviceKey: String
}

nonisolated struct LoginRequest: Encodable, Sendable {
    let gateToken: String
    let key: String
}

nonisolated struct OpenSessionRequest: Encodable, Sendable {
    let targetDeviceId: String
    let access: String
}

// MARK: - Local state

nonisolated enum LibraryView: Hashable, Sendable {
    case all
    case root
    case uncategorized
    case untagged
    case trash
    case folder(String)
    case category(String)
    case tag(String)

    var title: String {
        switch self {
        case .all: "全部素材"
        case .root: "根目录素材"
        case .uncategorized: "未分类素材"
        case .untagged: "未标签素材"
        case .trash: "回收站"
        case .folder(let path): path.split(separator: "/").last.map(String.init) ?? path
        case .category(let name): name
        case .tag(let name): name
        }
    }

    var isGlobal: Bool {
        switch self {
        case .all, .root, .uncategorized, .untagged: true
        default: false
        }
    }
}

nonisolated enum SizeFilter: Sendable, Equatable {
    case side(min: Int?, max: Int?)

    var label: String {
        switch self {
        case .side(let min, let max):
            if let min, let max { return "\(min)p – \(max)p" }
            if let min { return "≥ \(min)p" }
            if let max { return "≤ \(max)p" }
            return "尺寸"
        }
    }
}

nonisolated struct QueryState: Sendable, Equatable {
    var keywords: [String] = []
    var star: Int?
    var color: String?
    var size: SizeFilter?
    var orderBy: String = "modification_time"
    var order: String = "desc"

    var hasActiveFilters: Bool {
        star != nil || color != nil || size != nil
    }
}

nonisolated struct SortChoice: Identifiable, Hashable, Sendable {
    let id: String
    let label: String
    let orderBy: String
    let order: String

    static let all: [SortChoice] = [
        SortChoice(id: "mtime-desc", label: "修改时间 ↓", orderBy: "modification_time", order: "desc"),
        SortChoice(id: "mtime-asc", label: "修改时间 ↑", orderBy: "modification_time", order: "asc"),
        SortChoice(id: "name-asc", label: "名称 ↑", orderBy: "name", order: "asc"),
        SortChoice(id: "name-desc", label: "名称 ↓", orderBy: "name", order: "desc"),
        SortChoice(id: "size-desc", label: "大小 ↓", orderBy: "size", order: "desc"),
        SortChoice(id: "size-asc", label: "大小 ↑", orderBy: "size", order: "asc"),
        SortChoice(id: "star-desc", label: "评分 ↓", orderBy: "star", order: "desc"),
        SortChoice(id: "star-asc", label: "评分 ↑", orderBy: "star", order: "asc"),
    ]

    static func current(_ query: QueryState) -> SortChoice? {
        all.first { $0.orderBy == query.orderBy && $0.order == query.order }
    }
}

nonisolated struct LibraryCounts: Sendable, Equatable {
    var all: Int = 0
    var root: Int = 0
    var uncategorized: Int = 0
    var untagged: Int = 0
    var trash: Int = 0
}

// MARK: - Helpers

nonisolated enum ItemKey {
    static let trashPrefix = ".hawk/trash/"

    static func displayPath(_ path: String) -> String {
        path.hasPrefix(trashPrefix) ? String(path.dropFirst(trashPrefix.count)) : path
    }

    static func fileName(path: String, fallback: String = "") -> String {
        let visible = displayPath(path)
        let name = visible.split(separator: "/").last.map(String.init) ?? fallback
        return name.isEmpty ? fallback : name
    }

    static func baseName(path: String, fallback: String = "") -> String {
        let file = fileName(path: path, fallback: fallback)
        guard let dot = file.lastIndex(of: "."), dot != file.startIndex else { return file }
        return String(file[..<dot])
    }

    static func ext(path: String, fallback: String = "") -> String {
        let file = fileName(path: path, fallback: fallback)
        guard let dot = file.lastIndex(of: "."), dot < file.index(before: file.endIndex) else {
            return fallback.lowercased()
        }
        return String(file[file.index(after: dot)...]).lowercased()
    }
}
