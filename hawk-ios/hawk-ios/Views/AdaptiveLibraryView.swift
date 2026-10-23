import SwiftUI

struct AdaptiveLibraryView: View {
    @Environment(\.horizontalSizeClass) private var horizontalSizeClass
    let session: LibrarySession

    var body: some View {
        if horizontalSizeClass == .regular {
            NavigationSplitView {
                SidebarView(store: session.store, session: session)
                    .navigationSplitViewColumnWidth(min: 232, ideal: 260, max: 320)
            } detail: {
                LibraryScreen(store: session.store, session: session, showsSidebarButton: false)
            }
            .navigationSplitViewStyle(.balanced)
        } else {
            NavigationStack {
                LibraryScreen(store: session.store, session: session, showsSidebarButton: true)
            }
        }
    }
}
