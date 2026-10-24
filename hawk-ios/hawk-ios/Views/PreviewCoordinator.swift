import CoreGraphics
import Observation
import SwiftUI

/// 预览的呈现协调器：预览作为根层 overlay 而不是 fullScreenCover。
///
/// 为什么不用 fullScreenCover：它把呈现方（网格）移出视图层级，透明背景只会露出黑底，
/// 无法实现相册那种「下拉时遮罩淡出、露出背后的真实网格」。提到根层后网格一直在下面，
/// hero 转场（源格子 ↔ 全屏）由预览自己按源格子矩形驱动，等价于 Apple 相册的
/// 自定义容器转场 + 交互式驱动：抓源视图、呈现方保持可见、手势直接改转场进度。
@MainActor
@Observable
final class PreviewCoordinator {
    struct Session: Identifiable {
        let id = UUID()
        let index: Int
        let key: String
        let showsInfo: Bool
        /// 源格子在全屏坐标下的矩形（hero 转场的起点与回场终点）；未知时用中心缩放兜底
        let sourceFrame: CGRect?
        let context: LibraryContext
        let store: GridStore
    }

    private(set) var session: Session?
    /// 是否隐藏源格子缩略图：回场一开始就置 false，让缩略图在图片飞回期间提前重新就位
    private(set) var hidesSource = true

    func open(
        context: LibraryContext, store: GridStore, index: Int, showsInfo: Bool,
        sourceFrame: CGRect?
    ) {
        guard let item = store.item(at: index) else { return }
        hidesSource = true
        session = Session(
            index: index, key: item.key, showsInfo: showsInfo, sourceFrame: sourceFrame,
            context: context, store: store)
    }

    func close() {
        session = nil
        hidesSource = true
    }
}
