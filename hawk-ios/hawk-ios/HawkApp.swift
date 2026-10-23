import SwiftUI

@main struct HawkApp: App {
    @State private var app = AppModel()

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(app)
                .preferredColorScheme(.dark)
                .tint(HawkTheme.accent)
        }
    }
}
