import SwiftUI
import UIKit
import UniformTypeIdentifiers

/// 分享扩展入口：承载 SwiftUI 界面，读取分享进来的图片交给 ShareModel。
@objc(ShareViewController)
final class ShareViewController: UIViewController {
    private let model = ShareModel()

    override func viewDidLoad() {
        super.viewDidLoad()
        let host = UIHostingController(
            rootView: ShareView(
                model: model,
                onCancel: { [weak self] in self?.finish() },
                onDone: { [weak self] in self?.finish() }
            ))
        addChild(host)
        host.view.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(host.view)
        NSLayoutConstraint.activate([
            host.view.topAnchor.constraint(equalTo: view.topAnchor),
            host.view.bottomAnchor.constraint(equalTo: view.bottomAnchor),
            host.view.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            host.view.trailingAnchor.constraint(equalTo: view.trailingAnchor),
        ])
        host.didMove(toParent: self)

        let providers = Self.imageProviders(from: extensionContext)
        Task { @MainActor in
            await model.prepare(providers: providers)
        }
    }

    private func finish() {
        model.cleanup()
        extensionContext?.completeRequest(returningItems: [], completionHandler: nil)
    }

    private static func imageProviders(from context: NSExtensionContext?) -> [NSItemProvider] {
        let items = context?.inputItems.compactMap { $0 as? NSExtensionItem } ?? []
        return items.flatMap { $0.attachments ?? [] }
            .filter { $0.hasItemConformingToTypeIdentifier(UTType.image.identifier) }
    }
}
