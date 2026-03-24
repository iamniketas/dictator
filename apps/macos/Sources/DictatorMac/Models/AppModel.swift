import AppKit
import AVFoundation
import ApplicationServices
import Combine

// MARK: - Permission helpers

enum PermissionStatus: String {
    case unknown = "Unknown"
    case granted = "Granted"
    case denied  = "Denied"
}

@MainActor
final class PermissionState: ObservableObject {
    @Published var microphone: PermissionStatus = .unknown
    @Published var accessibility: PermissionStatus = .unknown

    func refresh() {
        switch AVCaptureDevice.authorizationStatus(for: .audio) {
        case .authorized:     microphone = .granted
        case .notDetermined:  microphone = .unknown
        default:              microphone = .denied
        }
        accessibility = AXIsProcessTrusted() ? .granted : .denied
    }

    func requestMicrophone() {
        AVCaptureDevice.requestAccess(for: .audio) { granted in
            Task { @MainActor in self.microphone = granted ? .granted : .denied }
        }
    }

    func requestAccessibilityPrompt() {
        let key = kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String
        _ = AXIsProcessTrustedWithOptions([key: true] as CFDictionary)
        refresh()
    }
}

// MARK: - AppModel

@MainActor
final class AppModel: ObservableObject {
    static let shared = AppModel()

    // MARK: Published state

    @Published var isRecording = false
    @Published var isTranscribing = false
    @Published var isStreamingChunkTranscribing = false
    @Published var isFinalizingStop = false
    @Published var isStreamingLoopActive = false

    @Published var recordingSeconds: Double = 0
    @Published var transcriptionElapsedSeconds: Double = 0
    @Published var postStopWaitSeconds: Double = 0
    @Published var lastPostStopWaitSeconds: Double = 0
    @Published var lastAudioDurationSeconds: Double = 0
    @Published var lastTranscriptionDurationSeconds: Double = 0
    @Published var lastRealtimeSpeedRatio: Double = 0
    @Published var lastStreamingSpeedupVsNormal: Double = 0
    @Published var streamingChunksProcessed = 0
    @Published var lastCaptureSamples = 0

    /// Live microphone RMS amplitude in [0, 1]. Published at ~30 fps during recording.
    @Published var micAmplitude: Float = 0

    @Published var statusMessage = "Idle"
    @Published var lastTranscript = "No transcript yet."
    @Published var lastSavedRecordingPath = ""
    @Published var lastSavedTranscriptPath = ""

    // Settings (mirrored from SettingsStore, kept in sync)
    @Published var streamingEnabled: Bool {
        didSet { settings.streamingEnabled = streamingEnabled }
    }
    @Published var chunkSeconds: Int {
        didSet { settings.chunkSeconds = chunkSeconds }
    }
    @Published var transcriptionLanguage: String {
        didSet { settings.language = transcriptionLanguage }
    }
    @Published var transcriptionBackend: TranscriptionBackend {
        didSet {
            settings.backend = transcriptionBackend
            rebuildTranscriptionService()
        }
    }
    @Published var whisperKitModelName: String {
        didSet {
            settings.whisperKitModelName = whisperKitModelName
            rebuildTranscriptionService()
        }
    }
    @Published var httpEndpointURL: String {
        didSet { settings.httpEndpointURL = httpEndpointURL }
    }
    @Published var mlxEndpointURL: String {
        didSet {
            settings.mlxEndpointURL = mlxEndpointURL
            rebuildTranscriptionService()
        }
    }
    @Published var mlxModelID: String {
        didSet {
            settings.mlxModelID = mlxModelID
            rebuildTranscriptionService()
        }
    }
    /// Tri-state: nil = not probed yet, true = reachable, false = unreachable.
    @Published var mlxServerReachable: Bool? = nil
    @Published var hotkeyMode: HotkeyManager.TriggerMode {
        didSet { settings.hotkeyModeRaw = hotkeyMode.rawValue }
    }
    @Published var llmEnabled: Bool {
        didSet { settings.llmEnabled = llmEnabled }
    }
    @Published var llmEndpointURL: String {
        didSet { settings.llmEndpointURL = llmEndpointURL }
    }
    @Published var llmModel: String {
        didSet { settings.llmModel = llmModel }
    }
    @Published var llmSystemPrompt: String {
        didSet { settings.llmSystemPrompt = llmSystemPrompt }
    }
    /// True while an LLM correction request is in flight.
    @Published var isCorrectingLLM = false
    /// Set to true after recording exceeds `longRecordingThreshold` seconds.
    /// Used to show a "tap again for hands-free" hint in the ticker.
    @Published var showLongRecordingHint = false
    let longRecordingThresholdSeconds: Double = 30

    // MARK: - Sub-services

    let permissions = PermissionState()
    let whisperKitService = WhisperKitTranscriptionService()

    private let settings = SettingsStore.shared
    private let audioCapture = AudioCaptureService()
    private let recordingArchive = RecordingArchiveService()
    private let textInjection = TextInjectionService()
    private var transcriptionService: TranscriptionService

    // MARK: - Internal state

    private var amplitudeTask: Task<Void, Never>?
    private var recordingTickerTask: Task<Void, Never>?
    private var transcriptionTickerTask: Task<Void, Never>?
    private var postStopTickerTask: Task<Void, Never>?
    private var streamingLoopTask: Task<Void, Never>?

    private var streamProcessedNativeIndex = 0
    private var streamingAccumulatedTranscript = ""
    private var postStopStartedAt: Date?
    private var normalModePostStopBaselineSeconds: Double = 0
    private var currentRecordingFileURL: URL?
    private var sourceAppPID: pid_t?
    private var didAutoPasteForCurrentRun = false

    // MARK: - Init

    private init() {
        // Load from SettingsStore
        streamingEnabled  = settings.streamingEnabled
        chunkSeconds      = settings.chunkSeconds
        transcriptionLanguage = settings.language
        transcriptionBackend  = settings.backend
        whisperKitModelName   = settings.whisperKitModelName
        httpEndpointURL       = settings.httpEndpointURL
        mlxEndpointURL        = settings.mlxEndpointURL
        mlxModelID            = settings.mlxModelID
        hotkeyMode        = HotkeyManager.TriggerMode(rawValue: settings.hotkeyModeRaw) ?? .smart
        llmEnabled        = settings.llmEnabled
        llmEndpointURL    = settings.llmEndpointURL
        llmModel          = settings.llmModel
        llmSystemPrompt   = settings.llmSystemPrompt

        // Build transcription service (MLX needs endpoint/model already set above)
        switch settings.backend {
        case .whisperKit: transcriptionService = whisperKitService
        case .mlx:        transcriptionService = MLXTranscriptionService(
                              endpointURL: settings.mlxEndpointURL, modelID: settings.mlxModelID)
        case .http:       transcriptionService = WhisperHTTPTranscriptionService()
        }
    }

    // MARK: - Transcription service factory

    private func makeTranscriptionService(backend: TranscriptionBackend) -> TranscriptionService {
        switch backend {
        case .whisperKit: return whisperKitService
        case .mlx:        return MLXTranscriptionService(endpointURL: mlxEndpointURL, modelID: mlxModelID)
        case .http:       return WhisperHTTPTranscriptionService()
        }
    }

    private func rebuildTranscriptionService() {
        transcriptionService = makeTranscriptionService(backend: transcriptionBackend)
    }

    /// Probes the MLX server and updates `mlxServerReachable`. Safe to call any time.
    func probeMlxServer() {
        Task { [weak self] in
            guard let self else { return }
            let reachable = await SharedTranscriptionServerConfig.probeMLC(url: mlxEndpointURL)
            await MainActor.run { self.mlxServerReachable = reachable }
        }
    }

    // MARK: - Toggle recording (called by HotkeyManager and tray button)

    func toggleRecording(sourceAppPID: pid_t? = nil) {
        if isRecording {
            guard !isFinalizingStop else { return }
            Task { await stopRecordingFlow() }
        } else {
            guard !isTranscribing, !isStreamingChunkTranscribing, !isFinalizingStop else { return }
            if let pid = sourceAppPID, pid != ProcessInfo.processInfo.processIdentifier {
                self.sourceAppPID = pid
            }
            startRecordingFlow()
        }
    }

    func stopRecordingIfActive() {
        guard isRecording, !isFinalizingStop else { return }
        Task { await stopRecordingFlow() }
    }

    // MARK: - Recording flow

    private func startRecordingFlow() {
        do {
            try audioCapture.startCapture()
        } catch {
            statusMessage = "Failed to start recording: \(error.localizedDescription)"
            return
        }

        isRecording = true
        isTranscribing = false
        isStreamingChunkTranscribing = false
        isStreamingLoopActive = false
        isFinalizingStop = false
        showLongRecordingHint = false
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
            let front = NSWorkspace.shared.frontmostApplication?.processIdentifier
            if let pid = front, pid != ProcessInfo.processInfo.processIdentifier {
                sourceAppPID = pid
            }
        }

        statusMessage = "Recording from microphone"
        lastTranscript = "Recording in progress..."
        startRecordingTicker()
        startAmplitudePolling()
        if streamingEnabled { startStreamingLoop() }
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
                streamingLoopTask?.cancel()
                streamingLoopTask = nil
                isStreamingLoopActive = false
                statusMessage = "Finalizing streaming transcription..."

                let waitStart = Date()
                while isStreamingChunkTranscribing {
                    if Date().timeIntervalSince(waitStart) > 15 {
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

            if let savedURL = try? recordingArchive.saveRecording(samples16kMono: result.samples16kMono) {
                lastSavedRecordingPath = savedURL.path
                currentRecordingFileURL = savedURL
            }

            if streamingEnabled {
                let trimmed = streamingAccumulatedTranscript.trimmingCharacters(in: .whitespacesAndNewlines)
                if trimmed.isEmpty {
                    statusMessage = "Streaming returned no text, running full transcription fallback..."
                    lastTranscript = "Streaming fallback: transcribing full audio..."
                    runTranscription(samples16k: result.samples16kMono,
                                     audioDuration: result.durationSeconds,
                                     mode: .streamingFallback)
                } else {
                    lastTranscriptionDurationSeconds = Date().timeIntervalSince(postStopStartedAt ?? Date())
                    if lastTranscriptionDurationSeconds > 0 {
                        lastRealtimeSpeedRatio = result.durationSeconds / lastTranscriptionDurationSeconds
                    }
                    completePostStopMeasurement(mode: .streaming)
                    statusMessage = "Streaming complete (wait \(formatSeconds(lastPostStopWaitSeconds)))"
                    lastTranscript = trimmed
                    injectAndPersist(text: trimmed, mode: .streaming, success: true, error: nil)
                    finishProcessingState()
                }
            } else {
                statusMessage = "Capture complete, transcribing..."
                lastTranscript = "Captured \(String(format: "%.1f", result.durationSeconds))s audio."
                runTranscription(samples16k: result.samples16kMono,
                                 audioDuration: result.durationSeconds,
                                 mode: .normal)
            }
        } catch {
            isRecording = false
            finishProcessingState()
            statusMessage = "Failed to stop recording"
            lastTranscript = "Audio stop error: \(error.localizedDescription)"
        }
    }

    // MARK: - Transcription dispatch

    private enum TranscriptionMode {
        case normal, streaming, streamingFallback

        var stringValue: String {
            switch self {
            case .normal:            return "normal"
            case .streaming:         return "streaming"
            case .streamingFallback: return "streaming_fallback"
            }
        }
    }

    private func runTranscription(samples16k: [Float], audioDuration: Double, mode: TranscriptionMode) {
        isTranscribing = true
        transcriptionElapsedSeconds = 0
        let transcribeStart = Date()
        startTranscriptionTicker(from: transcribeStart)

        Task { [weak self] in
            guard let self else { return }
            do {
                let text = try await transcriptionService.transcribe(
                    samples16kMono: samples16k,
                    language: transcriptionLanguage
                )
                await MainActor.run {
                    self.transcriptionTickerTask?.cancel()
                    self.transcriptionTickerTask = nil
                    self.isTranscribing = false
                    self.lastTranscriptionDurationSeconds = Date().timeIntervalSince(transcribeStart)
                    if self.lastTranscriptionDurationSeconds > 0 {
                        self.lastRealtimeSpeedRatio = audioDuration / self.lastTranscriptionDurationSeconds
                    }
                    self.completePostStopMeasurement(mode: mode)
                    self.statusMessage = "Transcription complete (wait \(self.formatSeconds(self.lastPostStopWaitSeconds)))"
                    self.lastTranscript = text.isEmpty ? "[Empty transcription]" : text
                    self.injectAndPersist(text: self.lastTranscript, mode: mode, success: true, error: nil)
                    self.finishProcessingState()
                }
            } catch {
                await MainActor.run {
                    self.transcriptionTickerTask?.cancel()
                    self.transcriptionTickerTask = nil
                    self.isTranscribing = false
                    self.lastTranscriptionDurationSeconds = Date().timeIntervalSince(transcribeStart)
                    self.completePostStopMeasurement(mode: mode)
                    self.statusMessage = "Transcription failed"
                    self.lastTranscript = "Error: \(error.localizedDescription)"
                    self.injectAndPersist(text: self.lastTranscript, mode: mode, success: false, error: error.localizedDescription)
                    self.finishProcessingState()
                }
            }
        }
    }

    // MARK: - Injection + persistence

    private func injectAndPersist(text: String, mode: TranscriptionMode, success: Bool, error: String?) {
        guard success, llmEnabled, !text.isEmpty, !text.hasPrefix("[") else {
            // No LLM — inject raw transcript immediately.
            performInjection(text: text, mode: mode, success: success, error: error)
            return
        }
        // Run LLM correction asynchronously; inject result when done.
        isCorrectingLLM = true
        statusMessage = "Correcting with LLM (\(llmModel))…"
        let pid = sourceAppPID
        sourceAppPID = nil
        let llm = LLMService(endpointURL: llmEndpointURL, model: llmModel)
        let prompt = LLMService.correctionPrompt(for: text, systemPrompt: llmSystemPrompt)
        Task { [weak self] in
            guard let self else { return }
            do {
                let corrected = try await llm.generate(prompt: prompt)
                let result = corrected.isEmpty ? text : corrected
                await MainActor.run {
                    self.isCorrectingLLM = false
                    self.lastTranscript = result
                    self.statusMessage = "LLM correction done"
                    if !self.didAutoPasteForCurrentRun {
                        self.textInjection.inject(result, into: pid)
                        self.didAutoPasteForCurrentRun = true
                    }
                    self.persistTranscript(text: result, mode: mode, success: true, errorMessage: nil)
                }
            } catch {
                await MainActor.run {
                    self.isCorrectingLLM = false
                    self.statusMessage = "LLM failed, using raw transcript"
                    // Fallback: inject uncorrected text
                    if !self.didAutoPasteForCurrentRun {
                        self.textInjection.inject(text, into: pid)
                        self.didAutoPasteForCurrentRun = true
                    }
                    self.persistTranscript(text: text, mode: mode, success: true, errorMessage: "LLM: \(error.localizedDescription)")
                }
            }
        }
    }

    private func performInjection(text: String, mode: TranscriptionMode, success: Bool, error: String?) {
        if !didAutoPasteForCurrentRun {
            textInjection.inject(text, into: sourceAppPID)
            didAutoPasteForCurrentRun = true
        }
        persistTranscript(text: text, mode: mode, success: success, errorMessage: error)
        sourceAppPID = nil
    }

    private func persistTranscript(text: String, mode: TranscriptionMode, success: Bool, errorMessage: String?) {
        guard let recordingURL = currentRecordingFileURL else { return }
        let endpoint = transcriptionBackend == .http ? httpEndpointURL : "whisperkit://\(whisperKitModelName)"
        _ = try? recordingArchive.saveTranscriptJSON(
            for: recordingURL,
            transcriptText: text,
            mode: mode.stringValue,
            language: transcriptionLanguage,
            endpoint: endpoint,
            audioSeconds: lastAudioDurationSeconds,
            postStopWaitSeconds: lastPostStopWaitSeconds,
            transcriptionSeconds: lastTranscriptionDurationSeconds > 0 ? lastTranscriptionDurationSeconds : nil,
            success: success,
            errorMessage: errorMessage
        )
    }

    // MARK: - Streaming loop

    private func startStreamingLoop() {
        streamingLoopTask?.cancel()
        isStreamingLoopActive = true
        streamingLoopTask = Task { [weak self] in
            guard let self else { return }
            while !Task.isCancelled {
                guard streamingEnabled, isRecording else { break }
                let chunkTarget = Double(max(chunkSeconds, 1))
                if unprocessedNativeDurationSeconds() >= chunkTarget {
                    await transcribeNextStreamingChunk(isFinal: false)
                    continue
                }
                try? await Task.sleep(for: .milliseconds(350))
            }
            await MainActor.run { self.isStreamingLoopActive = false }
        }
    }

    private func unprocessedNativeDurationSeconds() -> Double {
        let snap = audioCapture.snapshotNativeMono(from: streamProcessedNativeIndex)
        guard snap.sampleRate > 0 else { return 0 }
        return Double(snap.samples.count) / snap.sampleRate
    }

    private func transcribeNextStreamingChunk(isFinal: Bool) async {
        guard !isStreamingChunkTranscribing else { return }

        let snap = audioCapture.snapshotNativeMono(from: streamProcessedNativeIndex)
        streamProcessedNativeIndex = snap.nextIndex
        guard !snap.samples.isEmpty else { return }

        let chunk16k = AudioCaptureService.resampleTo16k(samples: snap.samples, nativeSampleRate: snap.sampleRate)
        let audioSec = Double(chunk16k.count) / 16_000.0
        guard isFinal || audioSec >= 0.2 else { return }

        isStreamingChunkTranscribing = true
        statusMessage = "Streaming chunk transcription..."

        do {
            let text = try await transcriptionService.transcribe(
                samples16kMono: chunk16k,
                language: transcriptionLanguage
            )
            streamingChunksProcessed += 1
            if !text.isEmpty {
                if !streamingAccumulatedTranscript.isEmpty { streamingAccumulatedTranscript.append(" ") }
                streamingAccumulatedTranscript.append(text)
                lastTranscript = streamingAccumulatedTranscript
            }
            statusMessage = "Streaming active (\(streamingChunksProcessed) chunks, ~\(formatSeconds(audioSec))/chunk)"
        } catch {
            statusMessage = "Streaming chunk failed: \(error.localizedDescription)"
        }
        isStreamingChunkTranscribing = false
    }

    // MARK: - Tickers

    private func startAmplitudePolling() {
        amplitudeTask?.cancel()
        amplitudeTask = Task { [weak self] in
            while let self, !Task.isCancelled {
                self.micAmplitude = self.audioCapture.currentRMS
                try? await Task.sleep(for: .milliseconds(33)) // ~30 fps
            }
        }
    }

    private func stopAmplitudePolling() {
        amplitudeTask?.cancel()
        amplitudeTask = nil
        micAmplitude = 0
    }

    private func startRecordingTicker() {
        recordingTickerTask?.cancel()
        showLongRecordingHint = false
        recordingTickerTask = Task { [weak self] in
            while let self, !Task.isCancelled {
                let elapsed = self.audioCapture.elapsedSeconds()
                self.recordingSeconds = elapsed
                if !self.showLongRecordingHint, elapsed >= self.longRecordingThresholdSeconds {
                    self.showLongRecordingHint = true
                }
                try? await Task.sleep(for: .milliseconds(200))
            }
        }
    }

    private func startTranscriptionTicker(from start: Date) {
        transcriptionTickerTask?.cancel()
        transcriptionTickerTask = Task { [weak self] in
            while let self, !Task.isCancelled {
                self.transcriptionElapsedSeconds = Date().timeIntervalSince(start)
                try? await Task.sleep(for: .milliseconds(200))
            }
        }
    }

    private func startPostStopTicker() {
        postStopTickerTask?.cancel()
        guard let started = postStopStartedAt else { return }
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

    // MARK: - Helpers

    private func finishProcessingState() {
        isFinalizingStop = false
        isTranscribing = false
        isStreamingChunkTranscribing = false
        isStreamingLoopActive = false
        stopAmplitudePolling()
        stopPostStopTicker()
    }

    private func completePostStopMeasurement(mode: TranscriptionMode) {
        guard let started = postStopStartedAt else { return }
        let waited = Date().timeIntervalSince(started)
        lastPostStopWaitSeconds = waited
        postStopWaitSeconds = waited

        switch mode {
        case .normal:
            normalModePostStopBaselineSeconds = normalModePostStopBaselineSeconds == 0
                ? waited
                : (normalModePostStopBaselineSeconds + waited) / 2.0
            lastStreamingSpeedupVsNormal = 0
        case .streaming, .streamingFallback:
            if normalModePostStopBaselineSeconds > 0, waited > 0 {
                lastStreamingSpeedupVsNormal = normalModePostStopBaselineSeconds / waited
            }
        }
        stopPostStopTicker()
    }

    private func formatSeconds(_ v: Double) -> String {
        "\(Int(max(0, v) * 10) / 10)s"
    }
}
