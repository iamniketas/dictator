import Carbon.HIToolbox
import CoreGraphics
import AppKit

/// Global hotkey manager.
///
/// Supports three trigger modes:
///  - **smart** (default): tap < 300 ms → toggle mode; hold ≥ 300 ms → PTT (stops on key release).
///  - **toggle**: every key-down flips the recording state.
///  - **ptt**: key-down starts, key-up stops.
///
/// Primary implementation uses `CGEventTap` (requires Accessibility permission).
/// Falls back to Carbon `RegisterEventHotKey` (toggle-only) if CGEventTap is unavailable.
///
/// Default hotkeys: **Cmd+Shift+D** and **Ctrl+Shift+D**.
@MainActor
final class HotkeyManager {

    enum TriggerMode: String {
        case smart  = "smart"
        case toggle = "toggle"
        case ptt    = "ptt"
    }

    /// Called when recording should start (or toggle, depending on mode).
    var onToggle: (() -> Void)?

    /// Called when recording should stop (PTT release or smart hold).
    var onStop: (() -> Void)?

    var mode: TriggerMode = .smart

    // MARK: - State

    private var eventTap: CFMachPort?
    private var runLoopSource: CFRunLoopSource?

    private var pressedAt: Date?
    private let holdThreshold: TimeInterval = 0.30

    // For toggle and smart-tap sub-mode: track whether we are recording.
    private var isRecording = false

    // Carbon fallback
    private var carbonEventHandler: EventHandlerRef?
    private var carbonHotKeyRefs: [EventHotKeyRef?] = []

    // Key codes
    private static let keyCodeD: Int64 = 2   // kVK_ANSI_D

    // MARK: - Lifecycle

    func start() {
        stop()
        if AXIsProcessTrusted() {
            startEventTap()
        } else {
            startCarbonFallback()
        }
    }

    func stop() {
        stopEventTap()
        stopCarbonFallback()
        pressedAt = nil
        isRecording = false
    }

    // MARK: - CGEventTap

    private func startEventTap() {
        let mask = CGEventMask(
            (1 << CGEventType.keyDown.rawValue) |
            (1 << CGEventType.keyUp.rawValue)
        )
        let selfPtr = Unmanaged.passUnretained(self).toOpaque()

        guard let tap = CGEvent.tapCreate(
            tap: .cgSessionEventTap,
            place: .headInsertEventTap,
            options: .defaultTap,
            eventsOfInterest: mask,
            callback: { _, type, event, refcon -> Unmanaged<CGEvent>? in
                guard let refcon else { return Unmanaged.passRetained(event) }
                let manager = Unmanaged<HotkeyManager>.fromOpaque(refcon).takeUnretainedValue()
                return manager.handleCGEvent(type: type, event: event)
            },
            userInfo: selfPtr
        ) else {
            // Accessibility not granted yet — fall back to Carbon.
            startCarbonFallback()
            return
        }

        let source = CFMachPortCreateRunLoopSource(kCFAllocatorDefault, tap, 0)
        CFRunLoopAddSource(CFRunLoopGetMain(), source, .commonModes)
        CGEvent.tapEnable(tap: tap, enable: true)

        eventTap = tap
        runLoopSource = source
    }

    private func stopEventTap() {
        if let tap = eventTap {
            CGEvent.tapEnable(tap: tap, enable: false)
            eventTap = nil
        }
        if let src = runLoopSource {
            CFRunLoopRemoveSource(CFRunLoopGetMain(), src, .commonModes)
            runLoopSource = nil
        }
    }

    /// Returns nil to consume the event (suppress from other apps), or the event to pass through.
    private func handleCGEvent(type: CGEventType, event: CGEvent) -> Unmanaged<CGEvent>? {
        let keyCode = event.getIntegerValueField(.keyboardEventKeycode)
        let flags = event.flags
        let hasCmdShift  = flags.contains([.maskCommand, .maskShift])
        let hasCtrlShift = flags.contains([.maskControl, .maskShift])
        guard keyCode == Self.keyCodeD, hasCmdShift || hasCtrlShift else {
            return Unmanaged.passRetained(event)
        }

        if type == .keyDown {
            DispatchQueue.main.async { self.handleKeyDown() }
        } else if type == .keyUp {
            DispatchQueue.main.async { self.handleKeyUp() }
        }
        return nil   // consume
    }

    // MARK: - Key event handling

    private func handleKeyDown() {
        switch mode {
        case .toggle:
            onToggle?()

        case .ptt:
            onToggle?()   // start
            isRecording = true

        case .smart:
            pressedAt = Date()
            if !isRecording {
                onToggle?()   // start
                isRecording = true
            } else {
                // Second tap while recording → toggle off immediately
                onToggle?()   // or onStop? — uses AppModel.toggleRecording which checks isRecording
                isRecording = false
                pressedAt = nil
            }
        }
    }

    private func handleKeyUp() {
        switch mode {
        case .toggle:
            break

        case .ptt:
            onStop?()
            isRecording = false

        case .smart:
            guard let t = pressedAt else { return }
            let held = Date().timeIntervalSince(t)
            pressedAt = nil
            if held >= holdThreshold {
                // Was a long press → PTT: stop on release
                onStop?()
                isRecording = false
            }
            // Short tap: recording continues; next keyDown will stop it (handled above).
        }
    }

    // MARK: - Carbon fallback (toggle-only, no Accessibility required)

    private func startCarbonFallback() {
        var eventSpec = EventTypeSpec(
            eventClass: OSType(kEventClassKeyboard),
            eventKind: UInt32(kEventHotKeyPressed)
        )
        let userData = UnsafeMutableRawPointer(Unmanaged.passUnretained(self).toOpaque())
        InstallEventHandler(
            GetApplicationEventTarget(),
            { _, eventRef, userData -> OSStatus in
                guard let eventRef, let userData else { return noErr }
                var hkID = EventHotKeyID()
                let status = GetEventParameter(
                    eventRef,
                    EventParamName(kEventParamDirectObject),
                    EventParamType(typeEventHotKeyID),
                    nil,
                    MemoryLayout<EventHotKeyID>.size,
                    nil,
                    &hkID
                )
                guard status == noErr else { return noErr }
                let manager = Unmanaged<HotkeyManager>.fromOpaque(userData).takeUnretainedValue()
                DispatchQueue.main.async { manager.onToggle?() }
                return noErr
            },
            1,
            &eventSpec,
            userData,
            &carbonEventHandler
        )

        registerCarbonHotkey(id: 1, modifiers: UInt32(cmdKey | shiftKey))
        registerCarbonHotkey(id: 2, modifiers: UInt32(controlKey | shiftKey))
    }

    private func stopCarbonFallback() {
        for ref in carbonHotKeyRefs { if let r = ref { UnregisterEventHotKey(r) } }
        carbonHotKeyRefs.removeAll()
        if let h = carbonEventHandler { RemoveEventHandler(h); carbonEventHandler = nil }
    }

    private func registerCarbonHotkey(id: UInt32, modifiers: UInt32) {
        var ref: EventHotKeyRef?
        let hkID = EventHotKeyID(signature: OSType(0x44494354), id: id)  // "DICT"
        let status = RegisterEventHotKey(
            UInt32(kVK_ANSI_D),
            modifiers,
            hkID,
            GetApplicationEventTarget(),
            0,
            &ref
        )
        if status == noErr { carbonHotKeyRefs.append(ref) }
    }
}
