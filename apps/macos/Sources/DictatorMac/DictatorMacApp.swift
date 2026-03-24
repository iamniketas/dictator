import SwiftUI

// No @main here — main.swift calls DictatorMacApp.main() directly.
struct DictatorMacApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate

    var body: some Scene {
        // Floating ticker window shown during recording / transcription.
        WindowGroup("Dictator", id: "dashboard") {
            DashboardView(model: AppModel.shared)
        }
        .windowResizability(.contentSize)
        .windowStyle(.hiddenTitleBar)

        // Native Settings window (Cmd+,).
        Settings {
            SettingsView(model: AppModel.shared, whisperKit: AppModel.shared.whisperKitService)
        }
    }
}
