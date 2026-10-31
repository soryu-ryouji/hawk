import Foundation

// API 契约：字段与 hawk-daemon 的 server-rest-api-v1.md / openapi.json 一一对应。
// 解码统一走 convertFromSnakeCase，编码统一走 convertToSnakeCase。

nonisolated struct AppInfo: Decodable, Sendable {
    let version: String
    let platform: String?
    let execPath: String?
    let role: String
    let channel: String
    let hasLibrary: Bool
    let lan: LanInfo?
}

nonisolated struct LanInfo: Decodable, Sendable {
    let active: Bool
    let port: Int?
    let error: String?
    let peerActive: Bool?
    let peerPort: Int?
    let peerError: String?
}

nonisolated struct StartupInfo: Decodable, Sendable {
    let status: String
    let phase: String?
    let processed: Int?
    let total: Int?
    let message: String?
}

/// 周期兜底扫描设置（`.hawk/config.toml` 的 `[scan]`）
nonisolated struct ScanInfo: Decodable, Sendable {
    let periodic: Bool
    let interval: Int?
}

nonisolated struct LibraryInfo: Decodable, Sendable {
    let name: String
    let path: String?
    let applicationVersion: String?
    let storageMode: String?
    let modificationTime: Int64?
    let scan: ScanInfo?
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

/// 锁快照：服务端只下发名称，密码哈希永不出库。
nonisolated struct LockSnapshot: Decodable, Sendable, Equatable {
    let folders: [String]
    let categories: [String]
    let tags: [String]

    static let empty = LockSnapshot(folders: [], categories: [], tags: [])

    var isEmpty: Bool { folders.isEmpty && categories.isEmpty && tags.isEmpty }
}

/// 后台任务积压进度（SSE `task.progress`，与 `app/status` 同一份快照）
nonisolated struct TaskProgress: Decodable, Sendable, Equatable {
    let task: String
    let pending: Int
    let active: Int
    let phase: String?
    let processed: Int?
    let total: Int?

    var isBusy: Bool { pending > 0 || active > 0 }

    var label: String {
        if let processed, let total, total > 0 {
            return "正在索引 \(processed)/\(total)"
        }
        return task == "thumbnail" ? "正在生成缩略图" : "正在索引素材"
    }
}

nonisolated struct TaskBacklog: Decodable, Sendable {
    let pending: Int
    let active: Int
}

nonisolated struct IndexBacklog: Decodable, Sendable {
    let pending: Int
    let active: Int
    let phase: String?
    let processed: Int?
    let total: Int?

    var isBusy: Bool { pending > 0 || active > 0 }
}

nonisolated struct TaskStatus: Decodable, Sendable {
    let thumbnail: TaskBacklog
    let index: IndexBacklog
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

nonisolated struct Item: Decodable, Sendable, Hashable {
    let id: String
    let path: String
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

    var key: String { "\(id)\n\(path)" }
    var displayName: String { ext.isEmpty ? name : "\(name).\(ext)" }
}

nonisolated struct PaletteColor: Decodable, Sendable, Hashable {
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
    let browserToken: String
    let role: String
    let deviceId: String
}

nonisolated struct ViewerSession: Decodable, Sendable {
    let sessionToken: String
    let role: String
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
    var categoriesMatch: String?
    var star: Int?
    var folders: [String]?
    var foldersExact: Bool?
    var withoutCategories: Bool?
    var withoutTags: Bool?
    var excludeFolders: [String]?
    var excludeCategories: [String]?
    var excludeTags: [String]?
    var ext: String?
    var annotation: String?
    var url: String?
    var color: String?
    var minSideGte: Int?
    var minSideLte: Int?
    var minWidth: Int?
    var maxWidth: Int?
    var minHeight: Int?
    var maxHeight: Int?
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
    let tenantKey: String
}

nonisolated struct LoginRequest: Encodable, Sendable {
    let gateToken: String
    let key: String
}

nonisolated struct OpenSessionRequest: Encodable, Sendable {
    let targetDeviceId: String
    let role: String
}

nonisolated struct UnlockRequest: Encodable, Sendable {
    let dimension: String
    let name: String
    let password: String
}

nonisolated struct UnlockResponse: Decodable, Sendable {
    let unlockToken: String
}

/// 视图排序偏好：服务端按 scope 存在 `.hawk/view.toml`，与桌面端同步。
nonisolated struct ViewSortPreference: Codable, Sendable, Equatable {
    let orderBy: String
    let order: String
}

nonisolated struct ViewPreferenceRequest: Encodable, Sendable {
    let scope: String
    let orderBy: String
    let order: String
}

/// SSE 事件：事件名是持久契约，负载按事件名单独解码。
nonisolated struct ServerEvent: Sendable {
    let name: String
    let data: Data
}

nonisolated struct ItemIDsPayload: Decodable, Sendable {
    let id: String?
    let ids: [String]?
}

nonisolated struct ItemsPayload: Decodable, Sendable {
    let items: [Item]
}

// MARK: - Local state

nonisolated enum LibraryView: Hashable, Sendable, Identifiable {
    case all
    case root
    case uncategorized
    case untagged
    case trash
    case folder(String)
    case category(String)
    case tag(String)

    var id: String {
        switch self {
        case .all: "all"
        case .root: "root"
        case .uncategorized: "uncategorized"
        case .untagged: "untagged"
        case .trash: "trash"
        case .folder(let path): "folder:\(path)"
        case .category(let name): "category:\(name)"
        case .tag(let name): "tag:\(name)"
        }
    }

    var title: String {
        switch self {
        case .all: "全部素材"
        case .root: "根目录素材"
        case .uncategorized: "未分类素材"
        case .untagged: "未标签素材"
        case .trash: "回收站"
        case .folder(let path): ItemKey.fileName(path: path)
        case .category(let name): name
        case .tag(let name): name
        }
    }

    /// 视图来源（导航副标题用）：告诉用户「这是哪个维度下的内容」。
    var kindLabel: String {
        switch self {
        case .all, .root: "素材库"
        case .uncategorized: "智能分组"
        case .untagged: "智能分组"
        case .trash: "智能分组"
        case .folder: "文件夹"
        case .category: "分类"
        case .tag: "标签"
        }
    }

    var icon: String {
        switch self {
        case .all: "photo.on.rectangle"
        case .root: "house"
        case .uncategorized: "tray"
        case .untagged: "tag.slash"
        case .trash: "trash"
        case .folder: "folder"
        case .category: "square.grid.2x2"
        case .tag: "tag"
        }
    }

    var isGlobal: Bool {
        switch self {
        case .all, .root, .uncategorized, .untagged: true
        default: false
        }
    }

    /// 服务端视图偏好的 scope（智能分组/回收站无 scope，不记忆排序）。
    var sortScope: String? {
        switch self {
        case .root: "folder:"
        case .folder(let path): "folder:\(path)"
        case .category(let name): "category:\(name)"
        case .tag(let name): "tag:\(name)"
        default: nil
        }
    }

    var lockTarget: LockTarget? {
        switch self {
        case .folder(let path): LockTarget(dimension: .folder, name: path)
        case .category(let name): LockTarget(dimension: .category, name: name)
        case .tag(let name): LockTarget(dimension: .tag, name: name)
        default: nil
        }
    }
}

nonisolated enum LockDimension: String, Codable, Sendable, CaseIterable {
    case folder
    case category
    case tag

    var title: String {
        switch self {
        case .folder: "文件夹"
        case .category: "分类"
        case .tag: "标签"
        }
    }
}

nonisolated struct LockTarget: Codable, Sendable, Hashable, Identifiable {
    let dimension: LockDimension
    let name: String

    var id: String { "\(dimension.rawValue):\(name)" }

    var displayName: String {
        guard dimension == .folder else { return name }
        return name.split(separator: "/").last.map(String.init) ?? name
    }
}

nonisolated enum LockCoverage {
    /// 文件夹锁覆盖自身与全部子孙；分类/标签为精确匹配。
    static func target(for view: LibraryView, in locks: LockSnapshot) -> LockTarget? {
        if locks.isEmpty { return nil }
        switch view {
        case .folder(let path):
            var current: String? = path
            while let candidate = current {
                if locks.folders.contains(candidate) {
                    return LockTarget(dimension: .folder, name: candidate)
                }
                current =
                    candidate.contains("/")
                    ? String(candidate[..<candidate.lastIndex(of: "/")!]) : nil
            }
            return nil
        case .category(let name):
            return locks.categories.contains(name)
                ? LockTarget(dimension: .category, name: name) : nil
        case .tag(let name):
            return locks.tags.contains(name) ? LockTarget(dimension: .tag, name: name) : nil
        default:
            return nil
        }
    }

    /// 侧栏行用：该文件夹是否被任一锁覆盖（含祖先锁）。
    static func folderIsLocked(_ path: String, in locks: LockSnapshot) -> Bool {
        target(for: .folder(path), in: locks) != nil
    }
}

nonisolated enum SizeFilter: Sendable, Equatable, Hashable {
    case side(min: Int?, max: Int?)

    var label: String {
        switch self {
        case .side(let min, let max):
            if let min, let max { return "短边 \(min)–\(max)p" }
            if let min { return "短边 ≥ \(min)p" }
            if let max { return "短边 ≤ \(max)p" }
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

    var sort: ViewSortPreference { ViewSortPreference(orderBy: orderBy, order: order) }

    mutating func apply(_ sort: ViewSortPreference) {
        orderBy = sort.orderBy
        order = sort.order
    }
}

nonisolated struct SortChoice: Identifiable, Hashable, Sendable {
    let id: String
    let label: String
    let orderBy: String
    let order: String

    static let all: [SortChoice] = [
        SortChoice(id: "mtime-desc", label: "最新修改", orderBy: "modification_time", order: "desc"),
        SortChoice(id: "mtime-asc", label: "最早修改", orderBy: "modification_time", order: "asc"),
        SortChoice(id: "name-asc", label: "名称 A→Z", orderBy: "name", order: "asc"),
        SortChoice(id: "name-desc", label: "名称 Z→A", orderBy: "name", order: "desc"),
        SortChoice(id: "size-desc", label: "最大文件", orderBy: "size", order: "desc"),
        SortChoice(id: "size-asc", label: "最小文件", orderBy: "size", order: "asc"),
        SortChoice(id: "star-desc", label: "评分从高到低", orderBy: "star", order: "desc"),
        SortChoice(id: "star-asc", label: "评分从低到高", orderBy: "star", order: "asc"),
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

    func count(for view: LibraryView) -> Int? {
        switch view {
        case .all: all
        case .root: root
        case .uncategorized: uncategorized
        case .untagged: untagged
        case .trash: trash
        default: nil
        }
    }
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

    /// 网格卡片标题：优先完整详情（name + ext），否则从路径推。
    static func displayName(item: SkeletonItem, detail: Item?) -> String {
        if let detail { return detail.displayName }
        let base = baseName(path: item.path)
        let ext = ext(path: item.path)
        return ext.isEmpty ? base : "\(base).\(ext)"
    }
}
