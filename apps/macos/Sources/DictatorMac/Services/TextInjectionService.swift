import AppKit
import CoreGraphics

/// Injects text into the active application via Cmd+V paste.
///
/// Flow:
///  1. Write text to NSPasteboard.
///  2. Activate the target app (by PID).
///  3. Synthesise Cmd+V via CGEvent (two event taps) and AppleScript fallback.
@MainActor
final class TextInjectionService {

    /// Writes `text` to the pasteboard and attempts to paste it into the app identified by `targetPID`.
    /// If `targetPID` is nil or matches our own process, the text is only written to the pasteboard.
    func inject(_ text: String, into targetPID: pid_t?) {
        let pb = NSPasteboard.general
        pb.clearContents()
        pb.setString(text, forType: .string)

        guard let pid = targetPID, pid != ProcessInfo.processInfo.processIdentifier else {
            return
        }
        attemptPaste(to: pid)
    }

    // MARK: - Private

    /// Active paste task — cancelled if a new injection starts before the previous one finishes.
    private var pasteTask: Task<Void, Never>?

    private func attemptPaste(to targetPID: pid_t) {
        pasteTask?.cancel()
        let targetApp = NSRunningApplication(processIdentifier: targetPID)
        _ = targetApp?.activate(options: [])

        // Sequential retry: waits for the target app to become frontmost, then pastes once.
        // Intervals are incremental so cumulative delays match 140/320/620/1000/2000 ms.
        pasteTask = Task { [weak self] in
            let intervals: [Duration] = [
                .milliseconds(140),
                .milliseconds(180),
                .milliseconds(300),
                .milliseconds(380),
                .milliseconds(1000),
            ]
            for (index, interval) in intervals.enumerated() {
                guard !Task.isCancelled else { return }
                try? await Task.sleep(for: interval)
                guard !Task.isCancelled, let self else { return }

                let isFrontmost = NSWorkspace.shared.frontmostApplication?.processIdentifier == targetPID
                let isLastAttempt = index == intervals.count - 1

                if isFrontmost {
                    // Target is active — paste via CGEvent and return immediately.
                    self.sendCmdV()
                    return
                }

                _ = targetApp?.activate(options: [])

                if isLastAttempt {
                    // Last resort: AppleScript can paste without requiring frontmost.
                    self.appleScriptPaste()
                }
            }
        }
    }

    private func sendCmdV() {
        guard let source = CGEventSource(stateID: .combinedSessionState) else { return }
        let keyCodeV: CGKeyCode = 9
        for tap in [CGEventTapLocation.cghidEventTap, .cgSessionEventTap] {
            let down = CGEvent(keyboardEventSource: source, virtualKey: keyCodeV, keyDown: true)
            down?.flags = .maskCommand
            let up = CGEvent(keyboardEventSource: source, virtualKey: keyCodeV, keyDown: false)
            up?.flags = .maskCommand
            down?.post(tap: tap)
            up?.post(tap: tap)
        }
    }

    private func appleScriptPaste() {
        let script = """
        tell application "System Events"
            keystroke "v" using command down
        end tell
        """
        var error: NSDictionary?
        NSAppleScript(source: script)?.executeAndReturnError(&error)
    }
}
