import AppKit
import Combine

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private let model = AppModel.shared
    private var statusItem: NSStatusItem?
    private var statusSummaryItem: NSMenuItem?
    private var recordItem: NSMenuItem?
    private var streamingItem: NSMenuItem?
    private var chunk3Item: NSMenuItem?
    private var chunk8Item: NSMenuItem?
    private var chunk15Item: NSMenuItem?
    private var hotkeyManager: HotkeyManager?
    private var cancellables = Set<AnyCancellable>()
    private var didConfigureTickerWindow = false

    // MARK: - App lifecycle

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.accessory)
        buildStatusItem()
        bindState()
        model.permissions.refresh()
        startHotkeyManager()

        DispatchQueue.main.async { [weak self] in
            self?.configureTickerWindowIfNeeded()
            self?.updateTickerWindowVisibility()
        }
    }

    // MARK: - Hotkey manager

    private func startHotkeyManager() {
        let manager = HotkeyManager()
        manager.mode = HotkeyManager.TriggerMode(rawValue: SettingsStore.shared.hotkeyModeRaw) ?? .smart
        manager.onToggle = { [weak self] in
            Task { @MainActor in
                let pid = NSWorkspace.shared.frontmostApplication?.processIdentifier
                self?.model.toggleRecording(sourceAppPID: pid)
            }
        }
        manager.onStop = { [weak self] in
            Task { @MainActor in
                self?.model.stopRecordingIfActive()
            }
        }
        manager.start()
        hotkeyManager = manager
    }

    // MARK: - Status item / menu

    private func buildStatusItem() {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        item.button?.image = statusIcon(isRecording: false, isProcessing: false)
        item.button?.imagePosition = .imageOnly
        item.button?.imageScaling = .scaleNone

        let menu = NSMenu()

        let summary = NSMenuItem(title: "Status: Idle", action: nil, keyEquivalent: "")
        summary.isEnabled = false
        menu.addItem(summary)

        let hotkeyInfo = NSMenuItem(title: "Hotkey: Cmd+Shift+D / Ctrl+Shift+D", action: nil, keyEquivalent: "")
        hotkeyInfo.isEnabled = false
        menu.addItem(hotkeyInfo)
        menu.addItem(.separator())

        let showTicker = NSMenuItem(title: "Show Transcript Ticker", action: #selector(showTickerWindow), keyEquivalent: "")
        showTicker.target = self
        menu.addItem(showTicker)
        menu.addItem(.separator())

        let record = NSMenuItem(title: "Start Recording", action: #selector(toggleRecording), keyEquivalent: "")
        record.target = self
        menu.addItem(record)

        let streaming = NSMenuItem(title: "Streaming Mode", action: #selector(toggleStreaming), keyEquivalent: "")
        streaming.target = self
        menu.addItem(streaming)

        // Chunk size submenu
        let chunkMenu = NSMenu()
        let c3  = menuItem("3 seconds",  action: #selector(setChunk3))
        let c8  = menuItem("8 seconds",  action: #selector(setChunk8))
        let c15 = menuItem("15 seconds", action: #selector(setChunk15))
        [c3, c8, c15].forEach { chunkMenu.addItem($0) }
        let chunkRoot = NSMenuItem(title: "Chunk Size", action: nil, keyEquivalent: "")
        menu.setSubmenu(chunkMenu, for: chunkRoot)
        menu.addItem(chunkRoot)

        menu.addItem(.separator())
        menu.addItem(menuItem("Settings...", action: #selector(openSettings)))
        menu.addItem(.separator())
        menu.addItem(menuItem("Quit Dictator", action: #selector(quitApp), key: "q"))

        statusSummaryItem = summary
        recordItem        = record
        streamingItem     = streaming
        chunk3Item        = c3
        chunk8Item        = c8
        chunk15Item       = c15
        statusItem        = item
        statusItem?.menu  = menu

        refreshMenuState()
    }

    private func menuItem(_ title: String, action: Selector, key: String = "") -> NSMenuItem {
        let item = NSMenuItem(title: title, action: action, keyEquivalent: key)
        item.target = self
        return item
    }

    // MARK: - State binding

    private func bindState() {
        model.$isRecording.sink { [weak self] _ in self?.refreshMenuState() }.store(in: &cancellables)
        model.$streamingEnabled.sink { [weak self] _ in self?.refreshMenuState() }.store(in: &cancellables)
        model.$statusMessage.sink { [weak self] _ in self?.refreshMenuState() }.store(in: &cancellables)
        model.$isTranscribing.sink { [weak self] _ in self?.refreshMenuState() }.store(in: &cancellables)
        model.$isStreamingChunkTranscribing.sink { [weak self] _ in self?.refreshMenuState() }.store(in: &cancellables)
        model.$isFinalizingStop.sink { [weak self] _ in self?.refreshMenuState() }.store(in: &cancellables)
        model.$chunkSeconds.sink { [weak self] _ in self?.refreshMenuState() }.store(in: &cancellables)

        // Apply hotkey mode changes live — no restart needed.
        model.$hotkeyMode
            .sink { [weak self] mode in self?.hotkeyManager?.mode = mode }
            .store(in: &cancellables)
    }

    private func refreshMenuState() {
        let isProcessing = model.isFinalizingStop || model.isTranscribing || model.isStreamingChunkTranscribing
        statusItem?.button?.image = statusIcon(isRecording: model.isRecording, isProcessing: isProcessing)
        recordItem?.title   = model.isRecording ? "Stop Recording" : "Start Recording"
        streamingItem?.state = model.streamingEnabled ? .on : .off
        chunk3Item?.state   = model.chunkSeconds == 3  ? .on : .off
        chunk8Item?.state   = model.chunkSeconds == 8  ? .on : .off
        chunk15Item?.state  = model.chunkSeconds == 15 ? .on : .off

        let shortState: String
        if model.isRecording       { shortState = "Recording..." }
        else if isProcessing        { shortState = "Transcribing..." }
        else                        { shortState = model.statusMessage }
        statusSummaryItem?.title = "Status: \(shortState)"

        updateTickerWindowVisibility()
    }

    // MARK: - Menu actions

    @objc private func toggleRecording() {
        model.toggleRecording(sourceAppPID: nil)
    }

    @objc private func toggleStreaming() {
        model.streamingEnabled.toggle()
    }

    @objc private func setChunk3()  { model.chunkSeconds = 3 }
    @objc private func setChunk8()  { model.chunkSeconds = 8 }
    @objc private func setChunk15() { model.chunkSeconds = 15 }

    @objc private func openSettings() {
        NSApp.activate(ignoringOtherApps: true)
        // Opens the SwiftUI Settings scene
        if #available(macOS 13.0, *) {
            NSApp.sendAction(Selector(("showSettingsWindow:")), to: nil, from: nil)
        } else {
            NSApp.sendAction(Selector(("showPreferencesWindow:")), to: nil, from: nil)
        }
    }

    @objc private func showTickerWindow() {
        configureTickerWindowIfNeeded()
        guard let window = tickerWindow() else { return }
        positionTickerWindow(window)
        NSApp.activate(ignoringOtherApps: true)
        window.makeKeyAndOrderFront(nil)
    }

    @objc private func quitApp() {
        hotkeyManager?.stop()
        NSApp.terminate(nil)
    }

    // MARK: - Ticker window management

    private func tickerWindow() -> NSWindow? {
        NSApp.windows.first(where: { $0.identifier?.rawValue == "dashboard" }) ?? NSApp.windows.first
    }

    private func configureTickerWindowIfNeeded() {
        guard !didConfigureTickerWindow, let window = tickerWindow() else { return }
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
        window.setContentSize(NSSize(width: 320, height: 36))
    }

    private func updateTickerWindowVisibility() {
        configureTickerWindowIfNeeded()
        guard let window = tickerWindow() else { return }
        let shouldShow = model.isRecording || model.isFinalizingStop
            || model.isTranscribing || model.isStreamingChunkTranscribing
        if shouldShow {
            positionTickerWindow(window)
            window.orderFront(nil)
        } else {
            window.orderOut(nil)
        }
    }

    private func positionTickerWindow(_ window: NSWindow) {
        guard let screen = NSScreen.main ?? window.screen else { return }
        let visible = screen.visibleFrame
        let size = window.frame.size
        window.setFrameOrigin(NSPoint(
            x: visible.maxX - size.width - 20,
            y: visible.maxY - size.height - 6
        ))
    }

    // MARK: - Status icon

    private func statusIcon(isRecording: Bool, isProcessing: Bool) -> NSImage? {
        if let custom = customStatusIcon(isRecording: isRecording, isProcessing: isProcessing) {
            return custom
        }
        let cfg = NSImage.SymbolConfiguration(pointSize: 16, weight: .semibold, scale: .small)
        guard let base = NSImage(systemSymbolName: "mic.circle.fill", accessibilityDescription: "Dictator")?
            .withSymbolConfiguration(cfg) else { return nil }

        if #available(macOS 12.0, *) {
            let colors: [NSColor] = isRecording ? [.systemRed, .white]
                : isProcessing ? [.systemOrange, .white]
                : [.systemGray, .labelColor]
            if let colored = base.withSymbolConfiguration(.init(paletteColors: colors)) {
                colored.isTemplate = false
                return colored
            }
        }
        base.isTemplate = true
        statusItem?.button?.contentTintColor = isRecording ? .systemRed : (isProcessing ? .systemOrange : .labelColor)
        return base
    }

    private func customStatusIcon(isRecording: Bool, isProcessing: Bool) -> NSImage? {
        let name = isRecording ? "status-recording" : isProcessing ? "status-processing" : "status-idle"
        let resourcesDir = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()       // UI/
            .deletingLastPathComponent()       // DictatorMac/
            .appendingPathComponent("Resources")
        for ext in ["png", "pdf"] {
            let url = resourcesDir.appendingPathComponent("\(name).\(ext)")
            if FileManager.default.fileExists(atPath: url.path), let img = NSImage(contentsOf: url) {
                img.size = NSSize(width: 16, height: 16)
                img.isTemplate = false
                return img
            }
        }
        return nil
    }
}
