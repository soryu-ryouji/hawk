import CoreGraphics

/// 齐行网格布局，算法与 hawk-app/web/src/domains/library/logic/layout.ts 一致：
/// 贪心装行，非末行按容器宽反推行高并夹紧到目标高度的 0.5×–1.75×；末行保持目标高度。
nonisolated enum GridLayout {
    static let gap: CGFloat = 10
    static let metaHeight: CGFloat = 48

    struct Cell: Identifiable, Hashable, Sendable {
        let index: Int
        let item: SkeletonItem
        let width: CGFloat
        let height: CGFloat

        var id: String { item.key }
    }

    struct Row: Identifiable, Hashable, Sendable {
        let id: String
        let cells: [Cell]
        let startIndex: Int
        let endIndex: Int
        let height: CGFloat

        var indexRange: Range<Int> { startIndex..<endIndex }
    }

    static func rows(_ items: [SkeletonItem], width: CGFloat, targetHeight: CGFloat) -> [Row] {
        guard width > 0, !items.isEmpty else { return [] }
        var rows: [Row] = []
        var pending: [(index: Int, item: SkeletonItem, ratio: CGFloat)] = []
        var ratiosSum: CGFloat = 0

        func flush(isLast: Bool) {
            guard !pending.isEmpty else { return }
            let totalGap = CGFloat(pending.count - 1) * gap
            let fitHeight = (width - totalGap) / ratiosSum
            let ideal =
                isLast
                ? targetHeight
                : min(max(fitHeight, targetHeight * 0.5), targetHeight * 1.75)
            let height = max(1, min(ideal, fitHeight).rounded(.down))
            let cells = pending.map { entry in
                Cell(
                    index: entry.index,
                    item: entry.item,
                    width: max(1, (height * entry.ratio).rounded()),
                    height: height
                )
            }
            rows.append(
                Row(
                    id: pending[0].item.key,
                    cells: cells,
                    startIndex: pending[0].index,
                    endIndex: pending[pending.count - 1].index + 1,
                    height: height + metaHeight
                )
            )
            pending.removeAll(keepingCapacity: true)
            ratiosSum = 0
        }

        for (index, item) in items.enumerated() {
            let ratio = CGFloat(item.aspectRatio)
            if !pending.isEmpty,
                (ratiosSum + ratio) * targetHeight + CGFloat(pending.count) * gap > width
            {
                flush(isLast: false)
            }
            pending.append((index, item, ratio))
            ratiosSum += ratio
        }
        flush(isLast: true)
        return rows
    }
}
