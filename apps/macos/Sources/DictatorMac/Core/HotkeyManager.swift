import Carbon.HIToolbox
import Foundation

final class HotkeyManager {
    private let onToggle: () -> Void
    private var eventHandler: EventHandlerRef?
    private var hotKeyRefs: [EventHotKeyRef?] = []

    init(onToggle: @escaping () -> Void) {
        self.onToggle = onToggle
    }

    deinit {
        stop()
    }

    func start() {
        stop()

        var eventSpec = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
        let userData = UnsafeMutableRawPointer(Unmanaged.passUnretained(self).toOpaque())
        InstallEventHandler(
            GetApplicationEventTarget(),
            { _, eventRef, userData in
                guard
                    let eventRef,
                    let userData
                else {
                    return noErr
                }
                var hotKeyID = EventHotKeyID()
                let status = GetEventParameter(
                    eventRef,
                    EventParamName(kEventParamDirectObject),
                    EventParamType(typeEventHotKeyID),
                    nil,
                    MemoryLayout<EventHotKeyID>.size,
                    nil,
                    &hotKeyID
                )
                guard status == noErr else {
                    return noErr
                }
                let instance = Unmanaged<HotkeyManager>.fromOpaque(userData).takeUnretainedValue()
                DispatchQueue.main.async {
                    instance.onToggle()
                }
                return noErr
            },
            1,
            &eventSpec,
            userData,
            &eventHandler
        )

        registerHotkey(id: 1, modifiers: UInt32(cmdKey | shiftKey))
        registerHotkey(id: 2, modifiers: UInt32(controlKey | shiftKey))
    }

    func stop() {
        for ref in hotKeyRefs {
            if let ref {
                UnregisterEventHotKey(ref)
            }
        }
        hotKeyRefs.removeAll()

        if let eventHandler {
            RemoveEventHandler(eventHandler)
            self.eventHandler = nil
        }
    }

    private func registerHotkey(id: UInt32, modifiers: UInt32) {
        var hotKeyRef: EventHotKeyRef?
        let hotKeyID = EventHotKeyID(signature: OSType(0x44494354), id: id) // "DICT"
        let status = RegisterEventHotKey(
            UInt32(kVK_ANSI_D),
            modifiers,
            hotKeyID,
            GetApplicationEventTarget(),
            0,
            &hotKeyRef
        )
        if status == noErr {
            hotKeyRefs.append(hotKeyRef)
        }
    }
}
