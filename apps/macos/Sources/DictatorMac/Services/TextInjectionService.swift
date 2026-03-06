import AppKit
import ApplicationServices
import Foundation

enum TextInjectionMode: String, CaseIterable, Identifiable {
    case pasteAndSend
    case clipboardOnly

    var id: String { rawValue }

    var title: String {
        switch self {
        case .pasteAndSend:
            return "Paste into Frontmost App"
        case .clipboardOnly:
            return "Copy to Clipboard Only"
        }
    }
}

final class TextInjectionService {
    func copyToPasteboard(_ text: String) {
        let pb = NSPasteboard.general
        pb.clearContents()
        pb.setString(text, forType: .string)
    }

    func attemptAutoPaste(to targetPID: pid_t, completion: @escaping () -> Void) {
        if appendTextViaAccessibility(to: targetPID) {
            completion()
            return
        }

        let targetApp = NSRunningApplication(processIdentifier: targetPID)
        _ = targetApp?.activate()

        let delays: [DispatchTimeInterval] = [
            .milliseconds(140),
            .milliseconds(320),
            .milliseconds(620),
            .seconds(1),
            .seconds(2),
            .seconds(3)
        ]

        for delay in delays {
            DispatchQueue.main.asyncAfter(deadline: .now() + delay) {
                if NSWorkspace.shared.frontmostApplication?.processIdentifier != targetPID {
                    _ = targetApp?.activate()
                }

                if let source = CGEventSource(stateID: .combinedSessionState) {
                    self.postCommandV(using: source, tap: .cghidEventTap)
                    self.postCommandV(using: source, tap: .cgSessionEventTap)
                }

                self.performAppleScriptPaste(targetPID: targetPID)
            }
        }

        DispatchQueue.main.asyncAfter(deadline: .now() + .seconds(4)) {
            completion()
        }
    }

    private func postCommandV(using source: CGEventSource, tap: CGEventTapLocation) {
        let keyCodeV: CGKeyCode = 9
        let down = CGEvent(keyboardEventSource: source, virtualKey: keyCodeV, keyDown: true)
        down?.flags = .maskCommand
        let up = CGEvent(keyboardEventSource: source, virtualKey: keyCodeV, keyDown: false)
        up?.flags = .maskCommand
        down?.post(tap: tap)
        up?.post(tap: tap)
    }

    private func performAppleScriptPaste(targetPID: pid_t) {
        let script = """
        tell application "System Events"
            set frontmost of first application process whose unix id is \(targetPID) to true
            keystroke "v" using command down
        end tell
        """
        var error: NSDictionary?
        NSAppleScript(source: script)?.executeAndReturnError(&error)
    }

    private func appendTextViaAccessibility(to targetPID: pid_t) -> Bool {
        guard let clipboardText = NSPasteboard.general.string(forType: .string),
              !clipboardText.isEmpty else {
            return false
        }

        let appElement = AXUIElementCreateApplication(targetPID)
        var focusedObject: AnyObject?
        let focusResult = AXUIElementCopyAttributeValue(
            appElement,
            kAXFocusedUIElementAttribute as CFString,
            &focusedObject
        )
        guard focusResult == .success,
              let focusedObject,
              CFGetTypeID(focusedObject) == AXUIElementGetTypeID() else {
            return false
        }

        let focusedElement = unsafeBitCast(focusedObject, to: AXUIElement.self)
        var currentValueObject: AnyObject?
        let valueResult = AXUIElementCopyAttributeValue(
            focusedElement,
            kAXValueAttribute as CFString,
            &currentValueObject
        )
        guard valueResult == .success else {
            return false
        }

        let currentText = currentValueObject as? String ?? ""
        let separator = separatorForAppend(current: currentText)
        let appended = currentText + separator + clipboardText
        let setResult = AXUIElementSetAttributeValue(
            focusedElement,
            kAXValueAttribute as CFString,
            appended as CFTypeRef
        )
        return setResult == .success
    }

    private func separatorForAppend(current: String) -> String {
        guard !current.isEmpty else { return "" }
        if current.hasSuffix("\n") || current.hasSuffix(" ") || current.hasSuffix("\t") {
            return ""
        }
        return " "
    }
}
