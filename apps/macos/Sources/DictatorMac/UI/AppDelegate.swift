import AppKit
import Combine
import Foundation
import SwiftUI

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private let model = AppModel.shared
    private var statusItem: NSStatusItem?
    private var statusSummaryItem: NSMenuItem?
    private var hotkeySummaryItem: NSMenuItem?
    private var recordItem: NSMenuItem?
    private var streamingItem: NSMenuItem?
    private var launchAtLoginItem: NSMenuItem?
    private var chunk3Item: NSMenuItem?
    private var chunk8Item: NSMenuItem?
    private var chunk15Item: NSMenuItem?
    private var chunkRootItem: NSMenuItem?
    private var shortcutMonitor: HotkeyManager?
    private var cancellables = Set<AnyCancellable>()
    private var didConfigureTickerWindow = false
    private var settingsWindow: NSWindow?

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.accessory)
        buildStatusItem()
        bindState()
        model.permissions.refresh()
        shortcutMonitor = HotkeyManager { [weak self] in
            Task { @MainActor in
                let pid = NSWorkspace.shared.frontmostApplication?.processIdentifier
                self?.model.toggleRecording(sourceAppPID: pid)
            }
        }
        shortcutMonitor?.start()
        DispatchQueue.main.async { [weak self] in
            self?.configureTickerWindowIfNeeded()
            self?.updateTickerWindowVisibility()
        }
    }

    private func buildStatusItem() {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        item.button?.image = statusIconImage(isRecording: false, isProcessing: false)
        item.button?.imagePosition = .imageOnly
        item.button?.imageScaling = .scaleNone

        let menu = NSMenu()
        menu.autoenablesItems = false

        let summary = NSMenuItem(title: "Status: Ready", action: nil, keyEquivalent: "")
        summary.isEnabled = false
        menu.addItem(summary)

        let hotkeySummary = NSMenuItem(title: "Shortcut: Cmd+Shift+D / Ctrl+Shift+D", action: nil, keyEquivalent: "")
        hotkeySummary.isEnabled = false
        menu.addItem(hotkeySummary)
        menu.addItem(.separator())

        let record = NSMenuItem(title: "Start Recording", action: #selector(toggleRecording), keyEquivalent: "")
        record.target = self
        menu.addItem(record)

        let streaming = NSMenuItem(title: "Live Transcription", action: #selector(toggleStreaming(_:)), keyEquivalent: "")
        streaming.target = self
        menu.addItem(streaming)

        let chunkMenu = NSMenu()
        let chunk3 = NSMenuItem(title: "3 seconds", action: #selector(setChunk3), keyEquivalent: "")
        chunk3.target = self
        chunkMenu.addItem(chunk3)
        let chunk8 = NSMenuItem(title: "8 seconds", action: #selector(setChunk8), keyEquivalent: "")
        chunk8.target = self
        chunkMenu.addItem(chunk8)
        let chunk15 = NSMenuItem(title: "15 seconds", action: #selector(setChunk15), keyEquivalent: "")
        chunk15.target = self
        chunkMenu.addItem(chunk15)
        let chunkRoot = NSMenuItem(title: "Chunk Duration", action: nil, keyEquivalent: "")
        menu.setSubmenu(chunkMenu, for: chunkRoot)
        menu.addItem(chunkRoot)

        let launchAtLogin = NSMenuItem(title: "Open at Login", action: #selector(toggleLaunchAtLogin), keyEquivalent: "")
        launchAtLogin.target = self
        menu.addItem(launchAtLogin)

        menu.addItem(.separator())

        let settingsItem = NSMenuItem(title: "Settings...", action: #selector(openSettings), keyEquivalent: ",")
        settingsItem.target = self
        menu.addItem(settingsItem)

        let quitItem = NSMenuItem(title: "Quit", action: #selector(quitApp), keyEquivalent: "q")
        quitItem.target = self
        menu.addItem(quitItem)

        self.statusSummaryItem = summary
        self.hotkeySummaryItem = hotkeySummary
        self.recordItem = record
        self.streamingItem = streaming
        self.launchAtLoginItem = launchAtLogin
        self.chunk3Item = chunk3
        self.chunk8Item = chunk8
        self.chunk15Item = chunk15
        self.chunkRootItem = chunkRoot
        self.statusItem = item
        self.statusItem?.menu = menu

        refreshMenuState()
    }

    private func bindState() {
        model.$isRecording
            .sink { [weak self] _ in self?.refreshMenuState() }
            .store(in: &cancellables)
        model.$streamingEnabled
            .sink { [weak self] _ in self?.refreshMenuState() }
            .store(in: &cancellables)
        model.$launchAtLogin
            .sink { [weak self] _ in self?.refreshMenuState() }
            .store(in: &cancellables)
        model.$statusMessage
            .sink { [weak self] _ in self?.refreshMenuState() }
            .store(in: &cancellables)
        model.$isStreamingChunkTranscribing
            .sink { [weak self] _ in self?.refreshMenuState() }
            .store(in: &cancellables)
        model.$isTranscribing
            .sink { [weak self] _ in self?.refreshMenuState() }
            .store(in: &cancellables)
        model.$isFinalizingStop
            .sink { [weak self] _ in self?.refreshMenuState() }
            .store(in: &cancellables)
        model.$chunkSeconds
            .sink { [weak self] _ in self?.refreshMenuState() }
            .store(in: &cancellables)
    }

    private func refreshMenuState() {
        recordItem?.title = model.isRecording ? "Stop Recording" : "Start Recording"
        streamingItem?.state = model.streamingEnabled ? .on : .off
        launchAtLoginItem?.state = model.launchAtLogin ? .on : .off
        chunk3Item?.state = model.chunkSeconds == 3 ? .on : .off
        chunk8Item?.state = model.chunkSeconds == 8 ? .on : .off
        chunk15Item?.state = model.chunkSeconds == 15 ? .on : .off
        chunkRootItem?.isEnabled = model.streamingEnabled
        hotkeySummaryItem?.title = "Shortcut: Cmd+Shift+D / Ctrl+Shift+D"
        let isProcessing = model.isFinalizingStop || model.isTranscribing || model.isStreamingChunkTranscribing
        statusItem?.button?.image = statusIconImage(isRecording: model.isRecording, isProcessing: isProcessing)

        let shortState: String
        if model.isRecording {
            shortState = "Recording..."
        } else if isProcessing {
            shortState = "Processing..."
        } else {
            shortState = model.statusMessage
        }
        statusSummaryItem?.title = "Status: \(shortState)"
        updateTickerWindowVisibility()
    }

    private func statusIconImage(isRecording: Bool, isProcessing: Bool) -> NSImage? {
        if let custom = customStatusIcon(isRecording: isRecording, isProcessing: isProcessing) {
            return custom
        }

        let baseConfig = NSImage.SymbolConfiguration(pointSize: 16, weight: .semibold, scale: .small)
        let symbolName = "mic.circle.fill"
        guard let base = NSImage(systemSymbolName: symbolName, accessibilityDescription: "Dictator")?
            .withSymbolConfiguration(baseConfig) else {
            return nil
        }
        if #available(macOS 12.0, *) {
            let colors: [NSColor]
            if isRecording {
                colors = [.systemRed, .white]
            } else if isProcessing {
                colors = [.systemOrange, .white]
            } else {
                colors = [.systemGray, .labelColor]
            }
            let palette = NSImage.SymbolConfiguration(
                paletteColors: colors
            )
            if let colored = base.withSymbolConfiguration(palette) {
                colored.isTemplate = false
                return colored
            }
        }

        base.isTemplate = true
        statusItem?.button?.contentTintColor = isRecording ? .systemRed : (isProcessing ? .systemOrange : .labelColor)
        return base
    }

    private func customStatusIcon(isRecording: Bool, isProcessing: Bool) -> NSImage? {
        let baseName: String
        if isRecording {
            baseName = "status-recording"
        } else if isProcessing {
            baseName = "status-processing"
        } else {
            baseName = "status-idle"
        }

        let resourcesDir = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("Resources", isDirectory: true)

        for ext in ["png", "pdf"] {
            let url = resourcesDir.appendingPathComponent("\(baseName).\(ext)")
            if FileManager.default.fileExists(atPath: url.path),
               let img = NSImage(contentsOf: url) {
                img.size = NSSize(width: 16, height: 16)
                img.isTemplate = false
                return img
            }
        }
        return nil
    }

    private func tickerWindow() -> NSWindow? {
        NSApp.windows.first
    }

    private func configureTickerWindowIfNeeded() {
        guard !didConfigureTickerWindow, let window = tickerWindow() else {
            return
        }
        didConfigureTickerWindow = true

        window.styleMask = [.borderless]
        window.titleVisibility = .hidden
        window.titlebarAppearsTransparent = true
        window.isMovableByWindowBackground = true
        window.level = .floating
        window.collectionBehavior.insert(.canJoinAllSpaces)
        window.collectionBehavior.insert(.fullScreenAuxiliary)
        window.isOpaque = false
        window.backgroundColor = .clear
        window.hasShadow = true
        window.setContentSize(NSSize(width: 170, height: 40))
    }

    private func updateTickerWindowVisibility() {
        configureTickerWindowIfNeeded()
        guard let window = tickerWindow() else {
            return
        }
        let shouldShow = model.isRecording || model.isFinalizingStop || model.isTranscribing || model.isStreamingChunkTranscribing
        if shouldShow {
            positionTickerWindow(window)
            window.orderFront(nil)
        } else {
            window.orderOut(nil)
        }
    }

    private func positionTickerWindow(_ window: NSWindow) {
        guard let screen = NSScreen.main ?? window.screen else {
            return
        }
        let visible = screen.visibleFrame
        let size = window.frame.size
        let x = visible.maxX - size.width - 20
        let y = visible.maxY - size.height - 6
        window.setFrameOrigin(NSPoint(x: x, y: y))
    }

    @objc private func openSettings() {
        openSettingsFallbackWindow()
    }

    private func openSettingsFallbackWindow() {
        if settingsWindow == nil {
            let window = NSWindow(
                contentRect: NSRect(x: 0, y: 0, width: 560, height: 620),
                styleMask: [.titled, .closable, .miniaturizable],
                backing: .buffered,
                defer: false
            )
            window.title = "Dictator Settings"
            window.center()
            window.isReleasedWhenClosed = false
            window.contentView = NSHostingView(rootView: SettingsView(model: model))
            settingsWindow = window
        }

        settingsWindow?.center()
        settingsWindow?.orderFrontRegardless()
        settingsWindow?.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    @objc private func toggleRecording() {
        model.toggleRecording(sourceAppPID: nil)
    }

    @objc private func toggleStreaming(_ sender: NSMenuItem) {
        let nextValue = sender.state != .on
        model.streamingEnabled = nextValue
        sender.state = nextValue ? .on : .off
        refreshMenuState()
    }

    @objc private func toggleLaunchAtLogin() {
        model.launchAtLogin.toggle()
    }

    @objc private func setChunk3() {
        model.chunkSeconds = 3
    }

    @objc private func setChunk8() {
        model.chunkSeconds = 8
    }

    @objc private func setChunk15() {
        model.chunkSeconds = 15
    }

    @objc private func quitApp() {
        shortcutMonitor?.stop()
        NSApp.terminate(nil)
    }
}
