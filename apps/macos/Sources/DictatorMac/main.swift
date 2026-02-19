import SwiftUI
import AppKit
import AVFoundation
import ApplicationServices
import Combine
import Foundation
import Carbon.HIToolbox

enum PermissionStatus: String {
    case unknown = "Unknown"
    case granted = "Granted"
    case denied = "Denied"
}

enum AudioCaptureError: LocalizedError {
    case alreadyRunning
    case notRunning

    var errorDescription: String? {
        switch self {
        case .alreadyRunning:
            return "Recording is already running"
        case .notRunning:
            return "Recording is not running"
        }
    }
}

struct AudioCaptureResult {
    let samples16kMono: [Float]
    let durationSeconds: Double
    let nativeSamplesCount: Int
}

enum TranscriptionError: LocalizedError {
    case badResponse
    case serverError(statusCode: Int, message: String)
    case invalidPayload

    var errorDescription: String? {
        switch self {
        case .badResponse:
            return "Invalid response from transcription server."
        case let .serverError(statusCode, message):
            return "Transcription server error \(statusCode): \(message)"
        case .invalidPayload:
            return "Transcription payload is missing text."
        }
    }
}

enum RecordingArchiveError: LocalizedError {
    case appSupportDirectoryUnavailable

    var errorDescription: String? {
        switch self {
        case .appSupportDirectoryUnavailable:
            return "Application Support directory is unavailable."
        }
    }
}

enum WAVEncoder {
    static func makeWAVData(samples: [Float], sampleRate: UInt32) -> Data {
        let channelCount: UInt16 = 1
        let bitsPerSample: UInt16 = 32
        let byteRate = sampleRate * UInt32(channelCount) * UInt32(bitsPerSample / 8)
        let blockAlign = channelCount * (bitsPerSample / 8)
        let dataSize = UInt32(samples.count * MemoryLayout<Float>.size)
        let riffSize = UInt32(36) + dataSize

        var data = Data()
        data.reserveCapacity(Int(44 + dataSize))

        data.append("RIFF".data(using: .ascii)!)
        data.appendUInt32LE(riffSize)
        data.append("WAVE".data(using: .ascii)!)

        data.append("fmt ".data(using: .ascii)!)
        data.appendUInt32LE(16)
        data.appendUInt16LE(3)
        data.appendUInt16LE(channelCount)
        data.appendUInt32LE(sampleRate)
        data.appendUInt32LE(byteRate)
        data.appendUInt16LE(blockAlign)
        data.appendUInt16LE(bitsPerSample)

        data.append("data".data(using: .ascii)!)
        data.appendUInt32LE(dataSize)
        for value in samples {
            data.appendFloat32LE(value)
        }

        return data
    }
}

final class RecordingArchiveService {
    private let fileManager = FileManager.default
    private let keepLastCount = 3

    func saveRecording(samples16kMono: [Float], sampleRate: UInt32 = 16_000) throws -> URL {
        let recordingsDirectory = try makeRecordingsDirectory()
        let filename = "recording-\(timestampString())-\(UUID().uuidString.prefix(8)).wav"
        let fileURL = recordingsDirectory.appendingPathComponent(filename)

        let wavData = WAVEncoder.makeWAVData(samples: samples16kMono, sampleRate: sampleRate)
        try wavData.write(to: fileURL, options: .atomic)
        try pruneRecordings(in: recordingsDirectory)
        return fileURL
    }

    func saveTranscriptJSON(
        for recordingFileURL: URL,
        transcriptText: String,
        mode: String,
        language: String,
        endpoint: String,
        audioSeconds: Double,
        postStopWaitSeconds: Double,
        transcriptionSeconds: Double?,
        success: Bool,
        errorMessage: String?
    ) throws -> URL {
        let jsonURL = recordingFileURL.deletingPathExtension().appendingPathExtension("json")
        let payload: [String: Any] = [
            "created_at": ISO8601DateFormatter().string(from: Date()),
            "recording_file": recordingFileURL.lastPathComponent,
            "mode": mode,
            "language": language,
            "endpoint": endpoint,
            "audio_seconds": audioSeconds,
            "post_stop_wait_seconds": postStopWaitSeconds,
            "transcription_seconds": transcriptionSeconds as Any,
            "success": success,
            "error": errorMessage as Any,
            "text": transcriptText
        ]
        let data = try JSONSerialization.data(withJSONObject: payload, options: [.prettyPrinted, .withoutEscapingSlashes])
        try data.write(to: jsonURL, options: .atomic)
        return jsonURL
    }

    private func makeRecordingsDirectory() throws -> URL {
        guard let appSupport = fileManager.urls(for: .applicationSupportDirectory, in: .userDomainMask).first else {
            throw RecordingArchiveError.appSupportDirectoryUnavailable
        }

        let dir = appSupport
            .appendingPathComponent("Dictator", isDirectory: true)
            .appendingPathComponent("Recordings", isDirectory: true)
        try fileManager.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }

    private func pruneRecordings(in directory: URL) throws {
        let fileURLs = try fileManager.contentsOfDirectory(
            at: directory,
            includingPropertiesForKeys: [.contentModificationDateKey],
            options: [.skipsHiddenFiles]
        )

        let wavs = fileURLs.filter { $0.pathExtension.lowercased() == "wav" }
        if wavs.count <= keepLastCount {
            return
        }

        let sorted = try wavs.sorted { lhs, rhs in
            let lDate = try lhs.resourceValues(forKeys: [.contentModificationDateKey]).contentModificationDate ?? .distantPast
            let rDate = try rhs.resourceValues(forKeys: [.contentModificationDateKey]).contentModificationDate ?? .distantPast
            return lDate > rDate
        }

        for old in sorted.dropFirst(keepLastCount) {
            try? fileManager.removeItem(at: old)
            let siblingJSON = old.deletingPathExtension().appendingPathExtension("json")
            try? fileManager.removeItem(at: siblingJSON)
        }
    }

    private func timestampString() -> String {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = TimeZone.current
        formatter.dateFormat = "yyyyMMdd-HHmmss"
        return formatter.string(from: Date())
    }
}

final class WhisperHTTPTranscriptionService {
    func transcribe(samples16kMono: [Float], language: String, endpointURL: URL) async throws -> String {
        let wavData = WAVEncoder.makeWAVData(samples: samples16kMono, sampleRate: 16_000)
        let boundary = "Boundary-\(UUID().uuidString)"
        let requestBody = makeMultipartBody(wavData: wavData, language: language, boundary: boundary)
        let audioSeconds = Double(samples16kMono.count) / 16_000.0
        let timeoutSeconds = max(120, min(1_800, (audioSeconds * 2.0) + 60))

        var request = URLRequest(url: endpointURL)
        request.httpMethod = "POST"
        request.timeoutInterval = timeoutSeconds
        request.setValue("multipart/form-data; boundary=\(boundary)", forHTTPHeaderField: "Content-Type")
        request.httpBody = requestBody

        let (data, response): (Data, URLResponse)
        do {
            (data, response) = try await URLSession.shared.data(for: request)
        } catch let error as URLError where error.code == .timedOut {
            throw TranscriptionError.serverError(
                statusCode: 408,
                message: "Client timeout after \(Int(timeoutSeconds))s. Try streaming mode or a shorter recording."
            )
        }

        guard let http = response as? HTTPURLResponse else {
            throw TranscriptionError.badResponse
        }

        guard (200...299).contains(http.statusCode) else {
            let message = String(data: data, encoding: .utf8) ?? "Unknown server error"
            throw TranscriptionError.serverError(statusCode: http.statusCode, message: message)
        }

        guard
            let json = try JSONSerialization.jsonObject(with: data) as? [String: Any],
            let text = json["text"] as? String
        else {
            throw TranscriptionError.invalidPayload
        }

        return text.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private func makeMultipartBody(wavData: Data, language: String, boundary: String) -> Data {
        var body = Data()
        let lineBreak = "\r\n"

        body.append("--\(boundary)\(lineBreak)".data(using: .utf8)!)
        body.append("Content-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\(lineBreak)".data(using: .utf8)!)
        body.append("Content-Type: audio/wav\(lineBreak)\(lineBreak)".data(using: .utf8)!)
        body.append(wavData)
        body.append(lineBreak.data(using: .utf8)!)

        body.append("--\(boundary)\(lineBreak)".data(using: .utf8)!)
        body.append("Content-Disposition: form-data; name=\"language\"\(lineBreak)\(lineBreak)".data(using: .utf8)!)
        body.append("\(language)\(lineBreak)".data(using: .utf8)!)

        body.append("--\(boundary)--\(lineBreak)".data(using: .utf8)!)
        return body
    }
}

final class AudioCaptureService {
    private let engine = AVAudioEngine()
    private let lock = NSLock()

    private var nativeMonoSamples: [Float] = []
    private var nativeSampleRate: Double = 16_000
    private var isRunning = false
    private var startTime: Date?

    func startCapture() throws {
        lock.lock()
        defer { lock.unlock() }

        guard !isRunning else {
            throw AudioCaptureError.alreadyRunning
        }

        nativeMonoSamples.removeAll(keepingCapacity: true)

        let input = engine.inputNode
        let format = input.inputFormat(forBus: 0)
        nativeSampleRate = format.sampleRate

        input.removeTap(onBus: 0)
        input.installTap(onBus: 0, bufferSize: 2048, format: format) { [weak self] buffer, _ in
            self?.appendNativeMonoSamples(buffer: buffer)
        }

        engine.prepare()
        try engine.start()

        startTime = Date()
        isRunning = true
    }

    func stopCapture() throws -> AudioCaptureResult {
        lock.lock()
        defer { lock.unlock() }

        guard isRunning else {
            throw AudioCaptureError.notRunning
        }

        engine.inputNode.removeTap(onBus: 0)
        engine.stop()
        isRunning = false

        let native = nativeMonoSamples
        let sampleRate = nativeSampleRate
        nativeMonoSamples.removeAll(keepingCapacity: true)

        let downsampled = Self.resampleTo16k(samples: native, nativeSampleRate: sampleRate)
        let duration = Double(downsampled.count) / 16_000.0

        startTime = nil
        return AudioCaptureResult(
            samples16kMono: downsampled,
            durationSeconds: duration,
            nativeSamplesCount: native.count
        )
    }

    func elapsedSeconds() -> Double {
        lock.lock()
        defer { lock.unlock() }
        guard isRunning, let startTime else {
            return 0
        }
        return Date().timeIntervalSince(startTime)
    }

    func snapshotNativeMono(from startIndex: Int) -> (samples: [Float], nextIndex: Int, sampleRate: Double) {
        lock.lock()
        let safeStart = min(max(0, startIndex), nativeMonoSamples.count)
        let slice = Array(nativeMonoSamples[safeStart..<nativeMonoSamples.count])
        let nextIndex = nativeMonoSamples.count
        let sampleRate = nativeSampleRate
        lock.unlock()
        return (slice, nextIndex, sampleRate)
    }

    private func appendNativeMonoSamples(buffer: AVAudioPCMBuffer) {
        guard let data = buffer.floatChannelData else {
            return
        }

        let channels = Int(buffer.format.channelCount)
        let frames = Int(buffer.frameLength)

        if frames == 0 || channels == 0 {
            return
        }

        var mono = [Float](repeating: 0, count: frames)

        if channels == 1 {
            let channel = data[0]
            for i in 0..<frames {
                mono[i] = channel[i]
            }
        } else {
            for frame in 0..<frames {
                var sum: Float = 0
                for channel in 0..<channels {
                    sum += data[channel][frame]
                }
                mono[frame] = sum / Float(channels)
            }
        }

        lock.lock()
        nativeMonoSamples.append(contentsOf: mono)
        lock.unlock()
    }

    static func resampleTo16k(samples: [Float], nativeSampleRate: Double) -> [Float] {
        guard !samples.isEmpty else {
            return []
        }

        if abs(nativeSampleRate - 16_000.0) < 0.01 {
            return samples
        }

        let ratio = nativeSampleRate / 16_000.0
        let outCount = max(1, Int(Double(samples.count) / ratio))
        var output = [Float]()
        output.reserveCapacity(outCount)

        for outIndex in 0..<outCount {
            let srcPos = Double(outIndex) * ratio
            let srcIndex = Int(srcPos)

            if srcIndex + 1 < samples.count {
                let frac = Float(srcPos - Double(srcIndex))
                let value = samples[srcIndex] * (1 - frac) + samples[srcIndex + 1] * frac
                output.append(value)
            } else if srcIndex < samples.count {
                output.append(samples[srcIndex])
            }
        }

        return output
    }
}

@MainActor
final class PermissionState: ObservableObject {
    @Published var microphone: PermissionStatus = .unknown
    @Published var accessibility: PermissionStatus = .unknown

    func refresh() {
        switch AVCaptureDevice.authorizationStatus(for: .audio) {
        case .authorized:
            microphone = .granted
        case .notDetermined:
            microphone = .unknown
        default:
            microphone = .denied
        }

        accessibility = AXIsProcessTrusted() ? .granted : .denied
    }

    func requestMicrophone() {
        AVCaptureDevice.requestAccess(for: .audio) { granted in
            Task { @MainActor in
                self.microphone = granted ? .granted : .denied
            }
        }
    }

    func requestAccessibilityPrompt() {
        let key = kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String
        let options = [key: true] as CFDictionary
        _ = AXIsProcessTrustedWithOptions(options)
        refresh()
    }
}

@MainActor
final class AppModel: ObservableObject {
    static let shared = AppModel()

    @Published var isRecording = false
    @Published var streamingEnabled = false
    @Published var launchAtLogin = false
    @Published var chunkSeconds = 8
    @Published var transcriptionLanguage = "ru"
    @Published var transcriptionEndpoint = "http://127.0.0.1:5500/transcribe"

    @Published var recordingSeconds: Double = 0
    @Published var lastCaptureSamples: Int = 0
    @Published var isTranscribing = false
    @Published var isStreamingChunkTranscribing = false
    @Published var isFinalizingStop = false
    @Published var isStreamingLoopActive = false
    @Published var streamingChunksProcessed = 0
    @Published var transcriptionElapsedSeconds: Double = 0
    @Published var lastAudioDurationSeconds: Double = 0
    @Published var lastTranscriptionDurationSeconds: Double = 0
    @Published var lastRealtimeSpeedRatio: Double = 0

    @Published var postStopWaitSeconds: Double = 0
    @Published var lastPostStopWaitSeconds: Double = 0
    @Published var normalModePostStopBaselineSeconds: Double = 0
    @Published var lastStreamingSpeedupVsNormal: Double = 0
    @Published var lastSavedRecordingPath = ""
    @Published var lastSavedTranscriptPath = ""

    @Published var statusMessage = "Idle"
    @Published var lastTranscript = "No transcript yet."

    let permissions = PermissionState()

    private let audioCapture = AudioCaptureService()
    private let transcriber = WhisperHTTPTranscriptionService()
    private let recordingArchive = RecordingArchiveService()
    private var recordingTickerTask: Task<Void, Never>?
    private var transcriptionTickerTask: Task<Void, Never>?
    private var postStopTickerTask: Task<Void, Never>?
    private var streamingLoopTask: Task<Void, Never>?
    private var streamProcessedNativeIndex = 0
    private var streamingAccumulatedTranscript = ""
    private var postStopStartedAt: Date?
    private var currentRecordingFileURL: URL?
    private var sourceAppPID: pid_t?
    private var didAutoPasteForCurrentRun = false

    private init() {}

    func toggleRecording(sourceAppPID: pid_t? = nil) {
        if isRecording {
            // Stop must always work, even while streaming chunk is currently transcribing.
            if isFinalizingStop {
                return
            }
            Task { [weak self] in
                await self?.stopRecordingFlow()
            }
        } else {
            if isTranscribing || isStreamingChunkTranscribing || isFinalizingStop {
                return
            }
            if let sourceAppPID, sourceAppPID != ProcessInfo.processInfo.processIdentifier {
                self.sourceAppPID = sourceAppPID
            }
            startRecordingFlow()
        }
    }

    private func startRecordingFlow() {
        do {
            try audioCapture.startCapture()
            isRecording = true
            isTranscribing = false
            isStreamingChunkTranscribing = false
            isStreamingLoopActive = false
            recordingSeconds = 0
            transcriptionElapsedSeconds = 0
            postStopWaitSeconds = 0
            lastPostStopWaitSeconds = 0
            lastStreamingSpeedupVsNormal = 0
            lastCaptureSamples = 0
            streamingChunksProcessed = 0
            lastRealtimeSpeedRatio = 0
            streamProcessedNativeIndex = 0
            streamingAccumulatedTranscript = ""
            currentRecordingFileURL = nil
            didAutoPasteForCurrentRun = false
            if sourceAppPID == nil {
                let frontPID = NSWorkspace.shared.frontmostApplication?.processIdentifier
                if let frontPID, frontPID != ProcessInfo.processInfo.processIdentifier {
                    sourceAppPID = frontPID
                }
            }
            isFinalizingStop = false
            statusMessage = "Recording from microphone"
            lastTranscript = "Recording in progress..."
            startRecordingTicker()
            if streamingEnabled {
                startStreamingLoop()
            }
        } catch {
            statusMessage = "Failed to start recording"
            lastTranscript = "Audio start error: \(error.localizedDescription)"
        }
    }

    private func stopRecordingFlow() async {
        recordingTickerTask?.cancel()
        recordingTickerTask = nil
        isRecording = false
        isFinalizingStop = true
        statusMessage = "Stopping recording..."
        lastTranscript = "Transcribing..."
        postStopStartedAt = Date()
        startPostStopTicker()

        do {
            if streamingEnabled {
                if let task = streamingLoopTask {
                    task.cancel()
                }
                streamingLoopTask = nil
                isStreamingLoopActive = false
                statusMessage = "Finalizing streaming transcription..."
                let waitStartedAt = Date()
                while isStreamingChunkTranscribing {
                    if Date().timeIntervalSince(waitStartedAt) > 15 {
                        // Prevent indefinite wait on edge-case state desync.
                        isStreamingChunkTranscribing = false
                        break
                    }
                    try? await Task.sleep(for: .milliseconds(100))
                }
                await transcribeNextStreamingChunk(isFinal: true)
            }

            let result = try audioCapture.stopCapture()
            recordingSeconds = result.durationSeconds
            lastAudioDurationSeconds = result.durationSeconds
            lastCaptureSamples = result.samples16kMono.count
            do {
                let savedURL = try recordingArchive.saveRecording(samples16kMono: result.samples16kMono)
                lastSavedRecordingPath = savedURL.path
                currentRecordingFileURL = savedURL
            } catch {
                statusMessage = "Recording saved in memory only (archive error)."
            }

            if streamingEnabled {
                let trimmed = streamingAccumulatedTranscript.trimmingCharacters(in: .whitespacesAndNewlines)
                if trimmed.isEmpty {
                    statusMessage = "Streaming returned no text, running full transcription fallback..."
                    lastTranscript = "Streaming fallback: running full transcription..."
                    runTranscription(samples16k: result.samples16kMono, audioDurationSeconds: result.durationSeconds, mode: .streamingFallback)
                } else {
                    lastTranscriptionDurationSeconds = Date().timeIntervalSince(postStopStartedAt ?? Date())
                    lastRealtimeSpeedRatio = lastTranscriptionDurationSeconds > 0 ? result.durationSeconds / lastTranscriptionDurationSeconds : 0
                    completePostStopMeasurement(mode: .streaming)
                    statusMessage = "Streaming transcription complete (wait \(formatSeconds(lastPostStopWaitSeconds)))"
                    lastTranscript = trimmed
                    copyAndPasteTranscript(trimmed)
                    persistTranscript(text: trimmed, mode: .streaming, success: true, errorMessage: nil)
                    finishProcessingState()
                }
            } else {
                statusMessage = "Capture complete, transcribing..."
                lastTranscript = "Captured \(Int(result.durationSeconds * 10) / 10)s, \(result.samples16kMono.count) samples (16k mono)."
                runTranscription(samples16k: result.samples16kMono, audioDurationSeconds: result.durationSeconds, mode: .normal)
            }
        } catch {
            isRecording = false
            finishProcessingState()
            statusMessage = "Failed to stop recording"
            lastTranscript = "Audio stop error: \(error.localizedDescription)"
        }
    }

    private enum TranscriptionMode {
        case normal
        case streaming
        case streamingFallback
    }

    private func runTranscription(samples16k: [Float], audioDurationSeconds: Double, mode: TranscriptionMode) {
        guard let endpointURL = URL(string: transcriptionEndpoint) else {
            statusMessage = "Invalid transcription endpoint URL"
            finishProcessingState()
            return
        }

        isTranscribing = true
        isStreamingChunkTranscribing = false
        transcriptionElapsedSeconds = 0

        let transcribeStartedAt = Date()
        startTranscriptionTicker(from: transcribeStartedAt)

        Task { [weak self] in
            guard let self else {
                return
            }
            do {
                let text = try await transcriber.transcribe(
                    samples16kMono: samples16k,
                    language: transcriptionLanguage,
                    endpointURL: endpointURL
                )
                await MainActor.run {
                    self.transcriptionTickerTask?.cancel()
                    self.transcriptionTickerTask = nil
                    self.isTranscribing = false
                    self.lastTranscriptionDurationSeconds = Date().timeIntervalSince(transcribeStartedAt)
                    if self.lastTranscriptionDurationSeconds > 0 {
                        self.lastRealtimeSpeedRatio = audioDurationSeconds / self.lastTranscriptionDurationSeconds
                    }
                    self.completePostStopMeasurement(mode: mode)
                    self.statusMessage = "Transcription complete (wait \(self.formatSeconds(self.lastPostStopWaitSeconds)))"
                    self.lastTranscript = text.isEmpty ? "[Empty transcription]" : text
                    self.copyAndPasteTranscript(self.lastTranscript)
                    self.persistTranscript(text: self.lastTranscript, mode: mode, success: true, errorMessage: nil)
                    self.finishProcessingState()
                }
            } catch {
                await MainActor.run {
                    self.transcriptionTickerTask?.cancel()
                    self.transcriptionTickerTask = nil
                    self.isTranscribing = false
                    self.lastTranscriptionDurationSeconds = Date().timeIntervalSince(transcribeStartedAt)
                    if self.lastTranscriptionDurationSeconds > 0 {
                        self.lastRealtimeSpeedRatio = audioDurationSeconds / self.lastTranscriptionDurationSeconds
                    }
                    self.completePostStopMeasurement(mode: mode)
                    self.statusMessage = "Transcription failed"
                    self.lastTranscript = "Transcription error (\(self.transcriptionEndpoint)): \(error.localizedDescription)"
                    self.persistTranscript(text: self.lastTranscript, mode: mode, success: false, errorMessage: error.localizedDescription)
                    self.finishProcessingState()
                }
            }
        }
    }

    private func persistTranscript(text: String, mode: TranscriptionMode, success: Bool, errorMessage: String?) {
        guard let recordingURL = currentRecordingFileURL else {
            return
        }
        do {
            let modeString: String
            switch mode {
            case .normal:
                modeString = "normal"
            case .streaming:
                modeString = "streaming"
            case .streamingFallback:
                modeString = "streaming_fallback"
            }
            let url = try recordingArchive.saveTranscriptJSON(
                for: recordingURL,
                transcriptText: text,
                mode: modeString,
                language: transcriptionLanguage,
                endpoint: transcriptionEndpoint,
                audioSeconds: lastAudioDurationSeconds,
                postStopWaitSeconds: lastPostStopWaitSeconds,
                transcriptionSeconds: lastTranscriptionDurationSeconds > 0 ? lastTranscriptionDurationSeconds : nil,
                success: success,
                errorMessage: errorMessage
            )
            lastSavedTranscriptPath = url.path
        } catch {
            // Keep the main flow stable even if transcript archive write fails.
        }
    }

    private func copyAndPasteTranscript(_ text: String) {
        if didAutoPasteForCurrentRun {
            return
        }
        let pb = NSPasteboard.general
        pb.clearContents()
        pb.setString(text, forType: .string)

        guard let targetPID = sourceAppPID, targetPID != ProcessInfo.processInfo.processIdentifier else {
            didAutoPasteForCurrentRun = true
            return
        }

        attemptAutoPaste(to: targetPID)
    }

    private func attemptAutoPaste(to targetPID: pid_t) {
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

                self.performAppleScriptPaste()
            }
        }

        DispatchQueue.main.asyncAfter(deadline: .now() + .seconds(4)) {
            self.didAutoPasteForCurrentRun = true
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

    private func performAppleScriptPaste() {
        let script = """
        tell application "System Events"
            keystroke "v" using command down
        end tell
        """
        var error: NSDictionary?
        NSAppleScript(source: script)?.executeAndReturnError(&error)
    }

    private func finishProcessingState() {
        isFinalizingStop = false
        isTranscribing = false
        isStreamingChunkTranscribing = false
        isStreamingLoopActive = false
        stopPostStopTicker()
    }

    private func completePostStopMeasurement(mode: TranscriptionMode) {
        guard let started = postStopStartedAt else {
            return
        }

        let waited = Date().timeIntervalSince(started)
        lastPostStopWaitSeconds = waited
        postStopWaitSeconds = waited

        switch mode {
        case .normal:
            if normalModePostStopBaselineSeconds == 0 {
                normalModePostStopBaselineSeconds = waited
            } else {
                normalModePostStopBaselineSeconds = (normalModePostStopBaselineSeconds + waited) / 2.0
            }
            lastStreamingSpeedupVsNormal = 0
        case .streaming:
            if normalModePostStopBaselineSeconds > 0, waited > 0 {
                lastStreamingSpeedupVsNormal = normalModePostStopBaselineSeconds / waited
            }
        case .streamingFallback:
            if normalModePostStopBaselineSeconds > 0, waited > 0 {
                lastStreamingSpeedupVsNormal = normalModePostStopBaselineSeconds / waited
            }
        }

        stopPostStopTicker()
    }

    private func startStreamingLoop() {
        streamingLoopTask?.cancel()
        isStreamingLoopActive = true
        streamingLoopTask = Task { [weak self] in
            guard let self else { return }
            while !Task.isCancelled {
                if !streamingEnabled || !isRecording {
                    break
                }

                let chunkTarget = Double(max(chunkSeconds, 1))
                if unprocessedNativeDurationSeconds() >= chunkTarget {
                    await transcribeNextStreamingChunk(isFinal: false)
                    continue
                }

                try? await Task.sleep(for: .milliseconds(350))
            }
            await MainActor.run {
                self.isStreamingLoopActive = false
            }
        }
    }

    private func unprocessedNativeDurationSeconds() -> Double {
        let snapshot = audioCapture.snapshotNativeMono(from: streamProcessedNativeIndex)
        guard snapshot.sampleRate > 0 else {
            return 0
        }
        return Double(snapshot.samples.count) / snapshot.sampleRate
    }

    private func transcribeNextStreamingChunk(isFinal: Bool) async {
        if isStreamingChunkTranscribing {
            return
        }

        guard let endpointURL = URL(string: transcriptionEndpoint) else {
            statusMessage = "Invalid transcription endpoint URL"
            return
        }

        let snapshot = audioCapture.snapshotNativeMono(from: streamProcessedNativeIndex)
        streamProcessedNativeIndex = snapshot.nextIndex

        if snapshot.samples.isEmpty {
            return
        }

        let chunk16k = AudioCaptureService.resampleTo16k(samples: snapshot.samples, nativeSampleRate: snapshot.sampleRate)
        let audioSeconds = Double(chunk16k.count) / 16_000.0
        if !isFinal && audioSeconds < 0.2 {
            return
        }

        isStreamingChunkTranscribing = true
        statusMessage = "Streaming chunk transcription..."

        do {
            let text = try await transcriber.transcribe(
                samples16kMono: chunk16k,
                language: transcriptionLanguage,
                endpointURL: endpointURL
            )

            streamingChunksProcessed += 1

            if !text.isEmpty {
                if !streamingAccumulatedTranscript.isEmpty {
                    streamingAccumulatedTranscript.append(" ")
                }
                streamingAccumulatedTranscript.append(text)
                lastTranscript = streamingAccumulatedTranscript
            }

            statusMessage = "Streaming active (\(streamingChunksProcessed) chunks, ~\(formatSeconds(audioSeconds))/chunk)"
        } catch {
            statusMessage = "Streaming chunk failed: \(error.localizedDescription)"
        }

        isStreamingChunkTranscribing = false
    }

    private func startRecordingTicker() {
        recordingTickerTask?.cancel()
        recordingTickerTask = Task { [weak self] in
            while let self, !Task.isCancelled {
                self.recordingSeconds = self.audioCapture.elapsedSeconds()
                try? await Task.sleep(for: .milliseconds(200))
            }
        }
    }

    private func startTranscriptionTicker(from startDate: Date) {
        transcriptionTickerTask?.cancel()
        transcriptionTickerTask = Task { [weak self] in
            while let self, !Task.isCancelled {
                self.transcriptionElapsedSeconds = Date().timeIntervalSince(startDate)
                try? await Task.sleep(for: .milliseconds(200))
            }
        }
    }

    private func startPostStopTicker() {
        postStopTickerTask?.cancel()
        guard let started = postStopStartedAt else {
            return
        }

        postStopTickerTask = Task { [weak self] in
            while let self, !Task.isCancelled {
                self.postStopWaitSeconds = Date().timeIntervalSince(started)
                try? await Task.sleep(for: .milliseconds(100))
            }
        }
    }

    private func stopPostStopTicker() {
        postStopTickerTask?.cancel()
        postStopTickerTask = nil
        postStopStartedAt = nil
    }

    private func formatRatio(_ value: Double) -> String {
        let clamped = max(0, value)
        let rounded = Int(clamped * 100) / 100
        return "\(rounded)x realtime"
    }

    private func formatSeconds(_ value: Double) -> String {
        let rounded = Int(max(0, value) * 10) / 10
        return "\(rounded)s"
    }
}

final class GlobalShortcutMonitor {
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
                let instance = Unmanaged<GlobalShortcutMonitor>.fromOpaque(userData).takeUnretainedValue()
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
    private var shortcutMonitor: GlobalShortcutMonitor?
    private var cancellables = Set<AnyCancellable>()
    private var didConfigureTickerWindow = false

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.accessory)
        buildStatusItem()
        bindState()
        model.permissions.refresh()
        shortcutMonitor = GlobalShortcutMonitor { [weak self] in
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

        let summary = NSMenuItem(title: "Status: Idle", action: nil, keyEquivalent: "")
        summary.isEnabled = false
        menu.addItem(summary)

        let hotkeySummary = NSMenuItem(title: "Hotkey: Cmd+Shift+D / Ctrl+Shift+D", action: nil, keyEquivalent: "")
        hotkeySummary.isEnabled = false
        menu.addItem(hotkeySummary)
        menu.addItem(.separator())

        let openItem = NSMenuItem(title: "Open Transcript Ticker", action: #selector(openDashboard), keyEquivalent: "")
        openItem.target = self
        menu.addItem(openItem)

        menu.addItem(.separator())

        let record = NSMenuItem(title: "Start Recording", action: #selector(toggleRecording), keyEquivalent: "")
        record.target = self
        menu.addItem(record)

        let streaming = NSMenuItem(title: "Streaming Mode", action: #selector(toggleStreaming), keyEquivalent: "")
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
        let chunkRoot = NSMenuItem(title: "Chunk Size", action: nil, keyEquivalent: "")
        menu.setSubmenu(chunkMenu, for: chunkRoot)
        menu.addItem(chunkRoot)

        let launchAtLogin = NSMenuItem(title: "Launch at Login", action: #selector(toggleLaunchAtLogin), keyEquivalent: "")
        launchAtLogin.target = self
        menu.addItem(launchAtLogin)

        menu.addItem(.separator())

        let quitItem = NSMenuItem(title: "Quit Dictator", action: #selector(quitApp), keyEquivalent: "q")
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
        hotkeySummaryItem?.title = "Hotkey: Cmd+Shift+D / Ctrl+Shift+D"
        let isProcessing = model.isFinalizingStop || model.isTranscribing || model.isStreamingChunkTranscribing
        statusItem?.button?.image = statusIconImage(isRecording: model.isRecording, isProcessing: isProcessing)

        let shortState: String
        if model.isRecording {
            shortState = "Recording..."
        } else if isProcessing {
            shortState = "Transcribing..."
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
        window.setContentSize(NSSize(width: 320, height: 36))
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

    @objc private func openDashboard() {
        configureTickerWindowIfNeeded()
        if let window = tickerWindow() {
            positionTickerWindow(window)
            NSApp.activate(ignoringOtherApps: true)
            window.makeKeyAndOrderFront(nil)
        }
    }

    @objc private func toggleRecording() {
        model.toggleRecording(sourceAppPID: nil)
    }

    @objc private func toggleStreaming() {
        model.streamingEnabled.toggle()
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

struct DashboardView: View {
    @ObservedObject var model: AppModel

    var body: some View {
        HStack(spacing: 8) {
            Circle()
                .fill(model.isRecording ? Color.red : Color.secondary.opacity(0.35))
                .frame(width: 8, height: 8)

            Text(tickerText)
                .font(.system(size: 14, weight: .medium, design: .rounded))
                .lineLimit(1)
                .truncationMode(.head)
                .frame(maxWidth: .infinity, alignment: .trailing)
                .textSelection(.enabled)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .frame(width: 320, height: 36)
        .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
    }

    private var tickerText: String {
        if model.isRecording && !model.lastTranscript.isEmpty && model.lastTranscript != "Recording in progress..." {
            return model.lastTranscript.replacingOccurrences(of: "\n", with: " ")
        }
        if model.isRecording {
            return "Recording..."
        }
        if model.isFinalizingStop || model.isTranscribing || model.isStreamingChunkTranscribing {
            let waited = Int(model.postStopWaitSeconds * 10) / 10
            return "Transcribing... \(waited)s"
        }
        if model.lastTranscript.isEmpty || model.lastTranscript == "No transcript yet." {
            return model.statusMessage
        }
        return model.lastTranscript.replacingOccurrences(of: "\n", with: " ")
    }
}

struct SettingsView: View {
    @ObservedObject var model: AppModel

    var body: some View {
        Form {
            Toggle("Launch at Login", isOn: $model.launchAtLogin)
            Toggle("Enable Streaming by Default", isOn: $model.streamingEnabled)
            Picker("Chunk Size", selection: $model.chunkSeconds) {
                Text("3s").tag(3)
                Text("8s").tag(8)
                Text("15s").tag(15)
            }
            .pickerStyle(.segmented)
            TextField("Whisper endpoint URL", text: $model.transcriptionEndpoint)
            TextField("Transcription language", text: $model.transcriptionLanguage)
            Text("This settings screen is a skeleton for the future full feature set.")
                .foregroundStyle(.secondary)
        }
        .padding(20)
        .frame(width: 460)
    }
}

private extension Data {
    mutating func appendUInt16LE(_ value: UInt16) {
        var littleEndian = value.littleEndian
        Swift.withUnsafeBytes(of: &littleEndian) { append(contentsOf: $0) }
    }

    mutating func appendUInt32LE(_ value: UInt32) {
        var littleEndian = value.littleEndian
        Swift.withUnsafeBytes(of: &littleEndian) { append(contentsOf: $0) }
    }

    mutating func appendFloat32LE(_ value: Float) {
        var bitPattern = value.bitPattern.littleEndian
        Swift.withUnsafeBytes(of: &bitPattern) { append(contentsOf: $0) }
    }
}

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
