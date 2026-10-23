import AVFoundation
import SwiftUI
import UIKit

struct QRScannerSheet: View {
    @Environment(\.dismiss) private var dismiss
    let onCode: (String) -> Void
    @State private var error: String?

    var body: some View {
        NavigationStack {
            ZStack {
                Color.black.ignoresSafeArea()
                if let error {
                    VStack(spacing: 14) {
                        Image(systemName: "camera.fill")
                            .font(.system(size: 34))
                            .foregroundStyle(HawkTheme.fg1)
                        Text(error)
                            .font(.system(size: 13))
                            .foregroundStyle(HawkTheme.fg0)
                            .multilineTextAlignment(.center)
                            .padding(.horizontal, 32)
                    }
                } else {
                    QRScannerView(
                        onCode: { code in
                            onCode(code)
                            dismiss()
                        },
                        onError: { message in error = message }
                    )
                    .ignoresSafeArea()
                    scannerOverlay
                }
            }
            .navigationTitle("扫描配对二维码")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("关闭") { dismiss() }
                }
            }
        }
    }

    private var scannerOverlay: some View {
        VStack {
            Spacer()
            RoundedRectangle(cornerRadius: 18)
                .stroke(Color.white.opacity(0.9), lineWidth: 2)
                .frame(width: 230, height: 230)
            Spacer()
            Text("将桌面端生成的配对二维码放入取景框")
                .font(.system(size: 13))
                .foregroundStyle(.white)
                .padding(.horizontal, 16)
                .padding(.vertical, 10)
                .background(.black.opacity(0.55), in: Capsule())
                .padding(.bottom, 40)
        }
        .allowsHitTesting(false)
    }
}

struct QRScannerView: UIViewControllerRepresentable {
    let onCode: (String) -> Void
    let onError: (String) -> Void

    func makeUIViewController(context: Context) -> ScannerViewController {
        let controller = ScannerViewController()
        controller.onCode = onCode
        controller.onError = onError
        return controller
    }

    func updateUIViewController(_ uiViewController: ScannerViewController, context: Context) {}
}

final class ScannerViewController: UIViewController {
    var onCode: ((String) -> Void)?
    var onError: ((String) -> Void)?

    private let session = AVCaptureSession()
    private let previewLayer = AVCaptureVideoPreviewLayer()
    private var configured = false
    private var delivered = false

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .black
        previewLayer.videoGravity = .resizeAspectFill
        view.layer.addSublayer(previewLayer)
        requestCamera()
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        previewLayer.frame = view.bounds
    }

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        if configured, !session.isRunning {
            session.startRunning()
        }
    }

    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        if session.isRunning {
            session.stopRunning()
        }
    }

    private func requestCamera() {
        Task { @MainActor in
            let granted = await withCheckedContinuation { continuation in
                AVCaptureDevice.requestAccess(for: .video) { allowed in
                    continuation.resume(returning: allowed)
                }
            }
            guard granted else {
                onError?("没有相机权限，请在系统设置 → hawk → 相机中允许访问")
                return
            }
            configureSession()
        }
    }

    private func configureSession() {
        guard !configured else { return }
        guard let device = AVCaptureDevice.default(for: .video) else {
            onError?("当前设备无法使用相机")
            return
        }
        guard let input = try? AVCaptureDeviceInput(device: device), session.canAddInput(input)
        else {
            onError?("无法打开相机")
            return
        }
        session.addInput(input)

        let output = AVCaptureMetadataOutput()
        guard session.canAddOutput(output) else {
            onError?("无法启用二维码识别")
            return
        }
        session.addOutput(output)
        output.setMetadataObjectsDelegate(self, queue: .main)
        output.metadataObjectTypes = [.qr]

        previewLayer.session = session
        configured = true
        session.startRunning()
    }

    private func deliver(_ code: String) {
        guard !delivered else { return }
        delivered = true
        if session.isRunning {
            session.stopRunning()
        }
        onCode?(code)
    }
}

extension ScannerViewController: AVCaptureMetadataOutputObjectsDelegate {
    nonisolated func metadataOutput(
        _ output: AVCaptureMetadataOutput,
        didOutput metadataObjects: [AVMetadataObject],
        from connection: AVCaptureConnection
    ) {
        guard
            let object = metadataObjects.first as? AVMetadataMachineReadableCodeObject,
            let value = object.stringValue
        else {
            return
        }
        Task { @MainActor [weak self] in
            self?.deliver(value)
        }
    }
}
