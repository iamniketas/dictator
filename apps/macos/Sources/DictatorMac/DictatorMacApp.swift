import SwiftUI

@main
struct DictatorMacApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate

    var body: some Scene {
        WindowGroup("Dictator", id: "dashboard") {
            DashboardView(model: AppModel.shared)
        }
        .windowResizability(.contentSize)
        .windowStyle(.hiddenTitleBar)

        Settings {
            SettingsView(model: AppModel.shared)
        }
    }
}
