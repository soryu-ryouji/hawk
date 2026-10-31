import SwiftUI
import UIKit

/// 预览页里的缩放层：UIScrollView + UIImageView，双击/双指缩放与越界回弹都交给系统。
///
/// 外层轮播与下拉关闭仍是 SwiftUI 手势，两层不能同时抢单指拖动：
/// 1x 时停用 UIScrollView 的 pan（不能整个 `isScrollEnabled = false`，那样捏合也收不到触摸），
/// 单指手势落到外层；放大后重新启用 pan，单指变为图片平移，外层通过 `GestureMask.none` 让出。
struct ZoomableImage: UIViewRepresentable {
    let url: URL?
    let sessionToken: String
    let aspectRatio: Double
    /// 只有当前页需要：收场前由预览侧无动画归零，避免缩放与 hero 两层变换叠加
    var zoomController: ZoomController?
    var onZoomChanged: (Bool) -> Void
    var onSingleTap: () -> Void

    func makeUIView(context: Context) -> ZoomableScrollView {
        let view = ZoomableScrollView()
        view.onZoomChanged = onZoomChanged
        view.onSingleTap = onSingleTap
        view.configure(url: url, aspectRatio: aspectRatio, sessionToken: sessionToken)
        zoomController?.attach(view)
        return view
    }

    func updateUIView(_ view: ZoomableScrollView, context: Context) {
        view.onZoomChanged = onZoomChanged
        view.onSingleTap = onSingleTap
        view.configure(url: url, aspectRatio: aspectRatio, sessionToken: sessionToken)
        zoomController?.attach(view)
    }

    static func dismantleUIView(_ uiView: ZoomableScrollView, coordinator: ()) {
        uiView.cancelLoading()
    }

    /// `scaledToFit` 的尺寸：按原图比例塞进可用区域，宽高都确定。
    /// 预览的 hero 落点也用同一份计算，保证缩放层与源格子严格对齐。
    static func fittedSize(for aspectRatio: Double, in size: CGSize) -> CGSize {
        guard size.width > 0, size.height > 0 else { return size }
        let aspect = aspectRatio > 0 ? CGFloat(aspectRatio) : 1
        let viewAspect = size.width / size.height
        if aspect > viewAspect {
            return CGSize(width: size.width, height: size.width / aspect)
        }
        return CGSize(width: size.height * aspect, height: size.height)
    }
}

/// 预览侧与当前页缩放视图之间的桥：标记 `weak`，页面切换后旧视图可正常释放。
final class ZoomController {
    private weak var scrollView: ZoomableScrollView?

    func attach(_ scrollView: ZoomableScrollView) {
        self.scrollView = scrollView
    }

    /// 无动画归零缩放与平移（回场 hero 开始前调用）
    func reset() {
        scrollView?.resetZoom()
    }
}

/// UIScrollView 子类：自己当 delegate，处理缩放几何、居中与手势。
final class ZoomableScrollView: UIScrollView, UIScrollViewDelegate {
    private static let maxScale: CGFloat = 6
    private static let doubleTapScale: CGFloat = 2.5

    var onZoomChanged: ((Bool) -> Void)?
    var onSingleTap: (() -> Void)?

    private let imageView = UIImageView()
    private let spinner = UIActivityIndicatorView(style: .medium)
    private let failureIcon = UIImageView()
    private var aspectRatio: CGFloat = 1
    private var fittedSize: CGSize = .zero
    private var loadedURL: URL?
    private var didStartLoad = false
    private var loadTask: Task<Void, Never>?
    private var reportedZoomed = false

    override init(frame: CGRect) {
        super.init(frame: frame)
        delegate = self
        backgroundColor = .clear
        contentInsetAdjustmentBehavior = .never
        showsHorizontalScrollIndicator = false
        showsVerticalScrollIndicator = false
        scrollsToTop = false
        bounces = true
        bouncesZoom = true
        alwaysBounceHorizontal = false
        alwaysBounceVertical = false
        minimumZoomScale = 1
        maximumZoomScale = Self.maxScale
        // 1x 时关闭平移但不关闭整个滚动视图：isScrollEnabled = false 会让 scroll view
        // 不再接收触摸，双指捏合也一起失效。单独停用 pan，单指拖动交给外层轮播/下拉关闭。
        panGestureRecognizer.isEnabled = false

        imageView.backgroundColor = UIColor(HawkTheme.bg2)
        imageView.contentMode = .scaleAspectFit
        addSubview(imageView)

        spinner.color = UIColor(HawkTheme.fg1)
        spinner.hidesWhenStopped = true
        addSubview(spinner)

        failureIcon.image = UIImage(systemName: "photo")
        failureIcon.tintColor = UIColor(HawkTheme.fg1).withAlphaComponent(0.5)
        failureIcon.contentMode = .scaleAspectFit
        failureIcon.isHidden = true
        addSubview(failureIcon)

        let doubleTap = UITapGestureRecognizer(target: self, action: #selector(handleDoubleTap(_:)))
        doubleTap.numberOfTapsRequired = 2
        addGestureRecognizer(doubleTap)
        let singleTap = UITapGestureRecognizer(target: self, action: #selector(handleSingleTap))
        singleTap.require(toFail: doubleTap)
        addGestureRecognizer(singleTap)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    func configure(url: URL?, aspectRatio: Double, sessionToken: String) {
        let ratio = aspectRatio > 0 ? CGFloat(aspectRatio) : 1
        if ratio != self.aspectRatio {
            self.aspectRatio = ratio
            setNeedsLayout()
        }
        if !didStartLoad || url != loadedURL {
            didStartLoad = true
            loadedURL = url
            load(url)
        }
    }

    func cancelLoading() {
        loadTask?.cancel()
        loadTask = nil
    }

    /// 无动画回到 1x 并居中（回场 hero 开始前调用）
    func resetZoom() {
        setZoomScale(1, animated: false)
        updateCentering()
        updateZoomState()
    }

    // MARK: - 布局

    override func layoutSubviews() {
        super.layoutSubviews()
        let fitted = ZoomableImage.fittedSize(
            for: Double(aspectRatio), in: bounds.size)
        guard fitted.width > 1, fitted.height > 1 else { return }
        if fitted != fittedSize {
            // 视口尺寸变了（旋转/分屏）：旧的缩放与平移量对不上新几何，回到原始比例
            fittedSize = fitted
            if zoomScale != 1 {
                setZoomScale(1, animated: false)
            }
            imageView.frame = CGRect(origin: .zero, size: fitted)
            contentSize = fitted
        }
        spinner.center = CGPoint(x: bounds.midX, y: bounds.midY)
        failureIcon.frame = CGRect(
            x: bounds.midX - 16, y: bounds.midY - 16, width: 32, height: 32)
        updateCentering()
    }

    /// 图片小于视口的方向用对称 contentInset 居中；只有超出视口的方向才可平移。
    private func updateCentering() {
        guard bounds.width > 0, bounds.height > 0 else { return }
        let insetX = max(0, (bounds.width - contentSize.width) / 2)
        let insetY = max(0, (bounds.height - contentSize.height) / 2)
        let insets = UIEdgeInsets(top: insetY, left: insetX, bottom: insetY, right: insetX)
        if contentInset != insets {
            contentInset = insets
        }
        var target = contentOffset
        let minX = -insetX
        let maxX = max(minX, contentSize.width - bounds.width + insetX)
        let minY = -insetY
        let maxY = max(minY, contentSize.height - bounds.height + insetY)
        if minX == maxX { target.x = minX }
        if minY == maxY { target.y = minY }
        if target != contentOffset {
            setContentOffset(target, animated: false)
        }
    }

    // MARK: - 加载

    private func load(_ url: URL?) {
        loadTask?.cancel()
        imageView.image = nil
        failureIcon.isHidden = true
        guard let url else {
            spinner.stopAnimating()
            failureIcon.isHidden = false
            return
        }
        spinner.startAnimating()
        loadTask = Task { [weak self] in
            let image = await ImageStore.shared.loadWithRetry(for: url, maxPixelSize: 4096, sessionToken: sessionToken)
            guard !Task.isCancelled, let self else { return }
            self.spinner.stopAnimating()
            if let image {
                self.imageView.image = image
            } else {
                self.failureIcon.isHidden = false
            }
        }
    }

    // MARK: - 缩放 / 手势

    func viewForZooming(in scrollView: UIScrollView) -> UIView? {
        imageView
    }

    func scrollViewDidZoom(_ scrollView: UIScrollView) {
        updateCentering()
        updateZoomState()
    }

    func scrollViewDidEndZooming(
        _ scrollView: UIScrollView, with view: UIView?, atScale scale: CGFloat
    ) {
        updateZoomState()
    }

    /// 跨过 1x 时回调一次；放大后立即打开平移，让单指拖动图片可用
    private func updateZoomState() {
        let zoomed = zoomScale > 1.001
        panGestureRecognizer.isEnabled = zoomed
        if zoomed != reportedZoomed {
            reportedZoomed = zoomed
            onZoomChanged?(zoomed)
        }
    }

    @objc private func handleDoubleTap(_ gesture: UITapGestureRecognizer) {
        guard imageView.image != nil else { return }
        Haptics.tap()
        guard zoomScale <= 1.001 else {
            setZoomScale(1, animated: true)
            return
        }
        // 以双击点为锚放大：zoom(to:) 内部按 rect 的尺寸推算目标倍率
        let point = gesture.location(in: imageView)
        let target = Self.doubleTapScale
        var rect = CGRect(origin: .zero, size: imageView.bounds.size)
        rect.size.width /= target
        rect.size.height /= target
        rect.origin = CGPoint(x: point.x - rect.width / 2, y: point.y - rect.height / 2)
        zoom(to: rect, animated: true)
    }

    @objc private func handleSingleTap() {
        onSingleTap?()
    }
}
