import SwiftUI

struct AdaptiveLibraryView: View {
    @Environment(\.horizontalSizeClass) private var horizontalSizeClass
    let context: LibraryContext

    var body: some View {
        if horizontalSizeClass == .regular {
            PadLibraryView(context: context)
        } else {
            PhoneLibraryView(context: context)
        }
    }
}

// MARK: - iPhone

private enum LibraryTab: Hashable {
    case library
    case browse
}

private struct PhoneLibraryView: View {
    let context: LibraryContext
    @State private var tab: LibraryTab
    @State private var browsePath: [LibraryView]

    init(context: LibraryContext) {
        self.context = context
        #if DEBUG
            // 本地冒烟：直接落到指定 Tab / 文件夹，省去手动点击
            let env = ProcessInfo.processInfo.environment
            _tab = State(initialValue: env["HAWK_IOS_DEBUG_TAB"] == "browse" ? .browse : .library)
            if let folder = env["HAWK_IOS_DEBUG_FOLDER"], !folder.isEmpty {
                _browsePath = State(initialValue: [.folder(folder)])
            } else {
                _browsePath = State(initialValue: [])
            }
        #else
            _tab = State(initialValue: .library)
            _browsePath = State(initialValue: [])
        #endif
    }

    var body: some View {
        TabView(selection: $tab) {
            Tab("素材", systemImage: "photo.on.rectangle", value: LibraryTab.library) {
                NavigationStack {
                    GridScreen(context: context, view: .all, isRoot: true)
                }
            }
            Tab("浏览", systemImage: "folder", value: LibraryTab.browse) {
                NavigationStack(path: $browsePath) {
                    BrowseScreen(context: context, active: nil) { view in
                        browsePath.append(view)
                    }
                    .navigationDestination(for: LibraryView.self) { view in
                        GridScreen(context: context, view: view, isRoot: false)
                    }
                }
            }
        }
        .tabBarMinimizeBehavior(.onScrollDown)
    }
}

// MARK: - iPad

private struct PadLibraryView: View {
    let context: LibraryContext

    @State private var selection: LibraryView
    @State private var columnVisibility: NavigationSplitViewVisibility = .detailOnly

    init(context: LibraryContext) {
        self.context = context
        #if DEBUG
            if let folder = ProcessInfo.processInfo.environment["HAWK_IOS_DEBUG_FOLDER"],
                !folder.isEmpty
            {
                _selection = State(initialValue: .folder(folder))
            } else {
                _selection = State(initialValue: .all)
            }
        #else
            _selection = State(initialValue: .all)
        #endif
    }

    var body: some View {
        NavigationSplitView(columnVisibility: $columnVisibility) {
            NavigationStack {
                BrowseScreen(context: context, active: selection) { view in
                    selection = view
                }
            }
            .navigationSplitViewColumnWidth(min: 250, ideal: 290, max: 360)
        } detail: {
            NavigationStack {
                GridScreen(context: context, view: selection, isRoot: true)
                    // 换视图即换浏览状态；滚动位置由 context 记住，切回来仍在原处
                    .id(selection)
            }
        }
        .navigationSplitViewStyle(.balanced)
        .onGeometryChange(for: Bool.self) { proxy in
            proxy.size.width > proxy.size.height
        } action: { landscape in
            // 横屏常驻侧栏；竖屏收起，把宽度留给网格（detail 工具栏的系统按钮可随时拉出）
            columnVisibility = landscape ? .all : .detailOnly
        }
    }
}
