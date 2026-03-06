import AppKit
import AVFoundation
import ApplicationServices
import Combine
import Foundation

enum PermissionStatus: String {
    case unknown = "Unknown"
    case granted = "Granted"
    case denied = "Denied"
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

        Task { @MainActor in
            for delay in [0.4, 1.0, 2.0, 4.0] {
                try? await Task.sleep(nanoseconds: UInt64(delay * 1_000_000_000))
                self.refresh()
                if self.accessibility == .granted {
                    break
                }
            }
        }
    }
}

@MainActor
final class AppModel: ObservableObject {
    static let shared = AppModel()

    @Published var isRecording = false
    @Published var streamingEnabled = false {
        didSet { settingsStore.saveStreamingEnabled(streamingEnabled) }
    }
    @Published var launchAtLogin = false {
        didSet { settingsStore.saveLaunchAtLogin(launchAtLogin) }
    }
    @Published var chunkSeconds = 8 {
        didSet { settingsStore.saveChunkSeconds(chunkSeconds) }
    }
    @Published var transcriptionBackendPreference: TranscriptionBackendPreference = .auto {
        didSet { settingsStore.saveTranscriptionBackendPreference(transcriptionBackendPreference) }
    }
    @Published var textInjectionMode: TextInjectionMode = .pasteAndSend {
        didSet { settingsStore.saveTextInjectionMode(textInjectionMode) }
    }
    @Published var transcriptionLanguage = "ru" {
        didSet { settingsStore.saveTranscriptionLanguage(transcriptionLanguage) }
    }
    @Published var transcriptionEndpoint = "http://127.0.0.1:5500/transcribe" {
        didSet { settingsStore.saveTranscriptionEndpoint(transcriptionEndpoint) }
    }
    @Published var activeTranscriptionBackend = "http"

    @Published var recordingSeconds: Double = 0
    @Published var inputLevel: Double = 0
    @Published var waveformBars: [Double] = Array(repeating: 0, count: 48)
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
    @Published var estimatedProcessingRemainingSeconds: Double = 0
    @Published var estimatedProcessingTotalSeconds: Double = 0
    @Published var lastPostStopWaitSeconds: Double = 0
    @Published var normalModePostStopBaselineSeconds: Double = 0
    @Published var lastStreamingSpeedupVsNormal: Double = 0
    @Published var lastSavedRecordingPath = ""
    @Published var lastSavedTranscriptPath = ""

    @Published var statusMessage = "Idle"
    @Published var lastTranscript = "No text yet."
    @Published var lastInjectionStatus = "No output yet."

    let permissions = PermissionState()

    private let audioCapture = AudioCaptureService()
    private let httpTranscriber: TranscriptionService = WhisperHTTPTranscriptionService()
    private let whisperKitTranscriber: TranscriptionService = WhisperKitTranscriptionService()
    private let recordingArchive = RecordingArchiveService()
    private let streamingCoordinator = StreamingTranscriptionCoordinator()
    private let textInjection = TextInjectionService()
    private let settingsStore = SettingsStore()

    private var recordingTickerTask: Task<Void, Never>?
    private var transcriptionTickerTask: Task<Void, Never>?
    private var postStopTickerTask: Task<Void, Never>?
    private var postStopStartedAt: Date?
    private var currentProcessingMode: TranscriptionMode?
    private var averageNormalProcessingSeconds: Double = 0
    private var averageStreamingProcessingSeconds: Double = 0
    private var currentRecordingFileURL: URL?
    private var sourceAppPID: pid_t?
    private var didAutoPasteForCurrentRun = false

    private init() {
        streamingEnabled = settingsStore.loadStreamingEnabled(default: streamingEnabled)
        launchAtLogin = settingsStore.loadLaunchAtLogin(default: launchAtLogin)
        chunkSeconds = settingsStore.loadChunkSeconds(default: chunkSeconds)
        transcriptionBackendPreference = settingsStore.loadTranscriptionBackendPreference(default: transcriptionBackendPreference)
        textInjectionMode = settingsStore.loadTextInjectionMode(default: textInjectionMode)
        transcriptionLanguage = settingsStore.loadTranscriptionLanguage(default: transcriptionLanguage)
        transcriptionEndpoint = settingsStore.loadTranscriptionEndpoint(default: transcriptionEndpoint)
    }

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
            streamingCoordinator.reset()
            currentRecordingFileURL = nil
            didAutoPasteForCurrentRun = false
            if sourceAppPID == nil {
                let frontPID = NSWorkspace.shared.frontmostApplication?.processIdentifier
                if let frontPID, frontPID != ProcessInfo.processInfo.processIdentifier {
                    sourceAppPID = frontPID
                }
            }
            isFinalizingStop = false
            statusMessage = "Recording..."
            lastTranscript = "Recording..."
            startRecordingTicker()
            if streamingEnabled {
                startStreamingPipeline()
            }
        } catch {
            statusMessage = "Couldn't start recording"
            lastTranscript = "Audio capture failed: \(error.localizedDescription)"
        }
    }

    private func stopRecordingFlow() async {
        recordingTickerTask?.cancel()
        recordingTickerTask = nil
        inputLevel = 0
        waveformBars = Array(repeating: 0, count: 48)
        isRecording = false
        isFinalizingStop = true
        statusMessage = "Stopping recording..."
        lastTranscript = "Processing..."
        postStopStartedAt = Date()
        currentProcessingMode = streamingEnabled ? .streaming : .normal
        startPostStopTicker()

        do {
            var streamingFinalization: StreamingTranscriptionCoordinator.Finalization?
            if streamingEnabled {
                statusMessage = "Finalizing streaming..."
                streamingFinalization = await streamingCoordinator.stopLoopAndFinalize(
                    snapshotProvider: { [weak self] index in
                        self?.audioCapture.snapshotNativeMono(from: index) ?? ([], index, 16_000)
                    },
                    transcribeChunk: { [weak self] samples16k in
                        guard let self else {
                            throw TranscriptionError.backendUnavailable(message: "App state is unavailable")
                        }
                        return try await self.transcribeChunkForStreaming(samples16k: samples16k)
                    },
                    onEvent: { [weak self] event in
                        self?.handleStreamingEvent(event)
                    }
                )
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
                statusMessage = "Recording was kept in memory only."
            }

            if streamingEnabled {
                if case .needsFullFallback = streamingFinalization {
                    statusMessage = "No streaming text yet. Running full pass..."
                    lastTranscript = "Running full transcription..."
                    runTranscription(samples16k: result.samples16kMono, audioDurationSeconds: result.durationSeconds, mode: .streamingFallback)
                } else if case let .text(trimmed) = streamingFinalization {
                    lastTranscriptionDurationSeconds = Date().timeIntervalSince(postStopStartedAt ?? Date())
                    lastRealtimeSpeedRatio = lastTranscriptionDurationSeconds > 0 ? result.durationSeconds / lastTranscriptionDurationSeconds : 0
                    completePostStopMeasurement(mode: .streaming)
                    statusMessage = "Completed in \(formatSeconds(lastPostStopWaitSeconds))"
                    lastTranscript = trimmed
                    copyAndPasteTranscript(trimmed)
                    persistTranscript(text: trimmed, mode: .streaming, success: true, errorMessage: nil)
                    finishProcessingState()
                } else {
                    statusMessage = "Streaming unavailable. Running full pass..."
                    lastTranscript = "Running full transcription..."
                    runTranscription(samples16k: result.samples16kMono, audioDurationSeconds: result.durationSeconds, mode: .streamingFallback)
                }
            } else {
                statusMessage = "Captured. Processing..."
                lastTranscript = "Recorded \(Int(result.durationSeconds * 10) / 10)s. Processing..."
                runTranscription(samples16k: result.samples16kMono, audioDurationSeconds: result.durationSeconds, mode: .normal)
            }
        } catch {
            isRecording = false
            finishProcessingState()
            statusMessage = "Couldn't stop recording"
            lastTranscript = "Audio stop failed: \(error.localizedDescription)"
        }
    }

    private enum TranscriptionMode {
        case normal
        case streaming
        case streamingFallback
    }

    private func fallbackHTTPTranscriptionIfPossible(samples16kMono: [Float], language: String) async throws -> String? {
        guard let endpoint = URL(string: transcriptionEndpoint) else {
            return nil
        }
        let text = try await httpTranscriber.transcribe(
            samples16kMono: samples16kMono,
            language: language,
            endpointURL: endpoint
        )
        activeTranscriptionBackend = httpTranscriber.backendName
        return text
    }

    private func selectTranscriber() throws -> (service: TranscriptionService, endpoint: URL?) {
        let httpEndpoint = URL(string: transcriptionEndpoint)

        switch transcriptionBackendPreference {
        case .http:
            guard let httpEndpoint else {
                throw TranscriptionError.invalidEndpoint
            }
            return (httpTranscriber, httpEndpoint)
        case .whisperKit:
            if whisperKitTranscriber.isAvailable {
                return (whisperKitTranscriber, nil)
            }
            guard let httpEndpoint else {
                throw TranscriptionError.backendUnavailable(message: "WhisperKit unavailable and HTTP endpoint is invalid")
            }
            return (httpTranscriber, httpEndpoint)
        case .auto:
            if whisperKitTranscriber.isAvailable {
                return (whisperKitTranscriber, nil)
            }
            guard let httpEndpoint else {
                throw TranscriptionError.invalidEndpoint
            }
            return (httpTranscriber, httpEndpoint)
        }
    }

    private func runTranscription(samples16k: [Float], audioDurationSeconds: Double, mode: TranscriptionMode) {
        let selected: (service: TranscriptionService, endpoint: URL?)
        do {
            selected = try selectTranscriber()
            activeTranscriptionBackend = selected.service.backendName
        } catch {
            statusMessage = error.localizedDescription
            finishProcessingState()
            return
        }

        isTranscribing = true
        isStreamingChunkTranscribing = false
        transcriptionElapsedSeconds = 0
        currentProcessingMode = mode

        let transcribeStartedAt = Date()
        startTranscriptionTicker(from: transcribeStartedAt)

        Task { [weak self] in
            guard let self else {
                return
            }
            do {
                let timeout = self.transcriptionTimeoutSeconds(
                    backend: selected.service.backendName,
                    audioDurationSeconds: audioDurationSeconds,
                    isStreamingChunk: false
                )
                let text = try await self.transcribeWithTimeout(
                    service: selected.service,
                    samples16kMono: samples16k,
                    language: self.transcriptionLanguage,
                    endpointURL: selected.endpoint,
                    timeoutSeconds: timeout
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
                    self.statusMessage = "Completed in \(self.formatSeconds(self.lastPostStopWaitSeconds))"
                    self.lastTranscript = text.isEmpty ? "[Empty transcription]" : text
                    self.copyAndPasteTranscript(self.lastTranscript)
                    self.persistTranscript(text: self.lastTranscript, mode: mode, success: true, errorMessage: nil)
                    self.finishProcessingState()
                }
            } catch {
                if selected.service.backendName == "whisperkit",
                   let fallbackText = try? await self.fallbackHTTPTranscriptionIfPossible(
                       samples16kMono: samples16k,
                       language: self.transcriptionLanguage
                   ) {
                    await MainActor.run {
                        self.transcriptionTickerTask?.cancel()
                        self.transcriptionTickerTask = nil
                        self.isTranscribing = false
                        self.lastTranscriptionDurationSeconds = Date().timeIntervalSince(transcribeStartedAt)
                        if self.lastTranscriptionDurationSeconds > 0 {
                            self.lastRealtimeSpeedRatio = audioDurationSeconds / self.lastTranscriptionDurationSeconds
                        }
                        self.completePostStopMeasurement(mode: mode)
                        self.statusMessage = "Completed in \(self.formatSeconds(self.lastPostStopWaitSeconds))"
                        self.lastTranscript = fallbackText
                        self.copyAndPasteTranscript(self.lastTranscript)
                        self.persistTranscript(text: self.lastTranscript, mode: mode, success: true, errorMessage: nil)
                        self.finishProcessingState()
                    }
                    return
                }

                await MainActor.run {
                    self.transcriptionTickerTask?.cancel()
                    self.transcriptionTickerTask = nil
                    self.isTranscribing = false
                    self.lastTranscriptionDurationSeconds = Date().timeIntervalSince(transcribeStartedAt)
                    if self.lastTranscriptionDurationSeconds > 0 {
                        self.lastRealtimeSpeedRatio = audioDurationSeconds / self.lastTranscriptionDurationSeconds
                    }
                    self.completePostStopMeasurement(mode: mode)
                    self.statusMessage = "Couldn't transcribe"
                    self.lastTranscript = "Transcription failed: \(error.localizedDescription)"
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

        textInjection.copyToPasteboard(text)

        if textInjectionMode == .clipboardOnly {
            lastInjectionStatus = "Copied to clipboard."
            didAutoPasteForCurrentRun = true
            return
        }

        permissions.refresh()
        if permissions.accessibility != .granted {
            lastInjectionStatus = "Copied to clipboard. Accessibility is required to paste automatically."
            didAutoPasteForCurrentRun = true
            return
        }

        guard let targetPID = sourceAppPID, targetPID != ProcessInfo.processInfo.processIdentifier else {
            lastInjectionStatus = "Copied to clipboard. No target app was found."
            didAutoPasteForCurrentRun = true
            return
        }

        lastInjectionStatus = "Pasting into target app..."
        textInjection.attemptAutoPaste(to: targetPID) { [weak self] in
            self?.lastInjectionStatus = "Paste attempt finished."
            self?.didAutoPasteForCurrentRun = true
        }
    }

    private func finishProcessingState() {
        isFinalizingStop = false
        isTranscribing = false
        isStreamingChunkTranscribing = false
        isStreamingLoopActive = false
        estimatedProcessingRemainingSeconds = 0
        estimatedProcessingTotalSeconds = 0
        currentProcessingMode = nil
        streamingCoordinator.reset()
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
            if averageNormalProcessingSeconds == 0 {
                averageNormalProcessingSeconds = waited
            } else {
                averageNormalProcessingSeconds = (averageNormalProcessingSeconds * 0.7) + (waited * 0.3)
            }
            if normalModePostStopBaselineSeconds == 0 {
                normalModePostStopBaselineSeconds = waited
            } else {
                normalModePostStopBaselineSeconds = (normalModePostStopBaselineSeconds + waited) / 2.0
            }
            lastStreamingSpeedupVsNormal = 0
        case .streaming, .streamingFallback:
            if averageStreamingProcessingSeconds == 0 {
                averageStreamingProcessingSeconds = waited
            } else {
                averageStreamingProcessingSeconds = (averageStreamingProcessingSeconds * 0.7) + (waited * 0.3)
            }
            if normalModePostStopBaselineSeconds > 0, waited > 0 {
                lastStreamingSpeedupVsNormal = normalModePostStopBaselineSeconds / waited
            }
        }

        stopPostStopTicker()
    }

    private func startStreamingPipeline() {
        streamingCoordinator.startLoop(
            chunkSecondsProvider: { [weak self] in self?.chunkSeconds ?? 8 },
            isRecordingProvider: { [weak self] in self?.isRecording ?? false },
            snapshotProvider: { [weak self] index in
                self?.audioCapture.snapshotNativeMono(from: index) ?? ([], index, 16_000)
            },
            transcribeChunk: { [weak self] samples16k in
                guard let self else {
                    throw TranscriptionError.backendUnavailable(message: "App state is unavailable")
                }
                return try await self.transcribeChunkForStreaming(samples16k: samples16k)
            },
            onEvent: { [weak self] event in
                self?.handleStreamingEvent(event)
            }
        )
    }

    private func transcribeChunkForStreaming(samples16k: [Float]) async throws -> String {
        let selected = try selectTranscriber()
        activeTranscriptionBackend = selected.service.backendName

        do {
            let audioSeconds = Double(samples16k.count) / 16_000.0
            let timeout = transcriptionTimeoutSeconds(
                backend: selected.service.backendName,
                audioDurationSeconds: audioSeconds,
                isStreamingChunk: true
            )
            return try await transcribeWithTimeout(
                service: selected.service,
                samples16kMono: samples16k,
                language: transcriptionLanguage,
                endpointURL: selected.endpoint,
                timeoutSeconds: timeout
            )
        } catch {
            if selected.service.backendName == "whisperkit",
               let fallbackText = try await fallbackHTTPTranscriptionIfPossible(
                   samples16kMono: samples16k,
                   language: transcriptionLanguage
               ) {
                return fallbackText
            }
            throw error
        }
    }

    private func handleStreamingEvent(_ event: StreamingTranscriptionCoordinator.Event) {
        switch event {
        case let .loopStateChanged(active):
            isStreamingLoopActive = active
            if !active {
                isStreamingChunkTranscribing = false
            }
        case .chunkStarted:
            isStreamingChunkTranscribing = true
            statusMessage = "Streaming..."
        case let .chunkCompleted(chunksProcessed, audioSeconds, accumulatedText):
            isStreamingChunkTranscribing = false
            streamingChunksProcessed = chunksProcessed
            if !accumulatedText.isEmpty {
                lastTranscript = accumulatedText
            }
            statusMessage = "Streaming (\(chunksProcessed) chunks, ~\(formatSeconds(audioSeconds))/chunk)"
        case let .chunkFailed(errorMessage):
            isStreamingChunkTranscribing = false
            statusMessage = "Streaming error: \(errorMessage)"
        }
    }

    private func startRecordingTicker() {
        recordingTickerTask?.cancel()
        recordingTickerTask = Task { [weak self] in
            while let self, !Task.isCancelled {
                self.recordingSeconds = self.audioCapture.elapsedSeconds()
                self.inputLevel = self.audioCapture.currentInputLevel()
                self.waveformBars = self.audioCapture.currentWaveformBars(count: 48)
                try? await Task.sleep(for: .milliseconds(33))
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
                let elapsed = Date().timeIntervalSince(started)
                self.postStopWaitSeconds = elapsed

                let expectedTotal: Double
                switch self.currentProcessingMode {
                case .normal:
                    expectedTotal = self.averageNormalProcessingSeconds > 0 ? self.averageNormalProcessingSeconds : self.normalModePostStopBaselineSeconds
                case .streaming, .streamingFallback:
                    if self.averageStreamingProcessingSeconds > 0 {
                        expectedTotal = self.averageStreamingProcessingSeconds
                    } else if self.averageNormalProcessingSeconds > 0 {
                        expectedTotal = self.averageNormalProcessingSeconds * 0.65
                    } else {
                        expectedTotal = self.normalModePostStopBaselineSeconds * 0.65
                    }
                case .none:
                    expectedTotal = 0
                }

                self.estimatedProcessingTotalSeconds = max(expectedTotal, 0)
                self.estimatedProcessingRemainingSeconds = max(expectedTotal - elapsed, 0)
                try? await Task.sleep(for: .milliseconds(100))
            }
        }
    }

    private func stopPostStopTicker() {
        postStopTickerTask?.cancel()
        postStopTickerTask = nil
        postStopStartedAt = nil
    }

    private func transcriptionTimeoutSeconds(
        backend: String,
        audioDurationSeconds: Double,
        isStreamingChunk: Bool
    ) -> Double {
        if isStreamingChunk {
            let base = max(8, min(35, (audioDurationSeconds * 2.2) + 4))
            return backend == "whisperkit" ? max(base, 12) : base
        }

        let base = max(20, min(240, (audioDurationSeconds * 2.5) + 18))
        return backend == "whisperkit" ? max(base, 35) : base
    }

    private func transcribeWithTimeout(
        service: TranscriptionService,
        samples16kMono: [Float],
        language: String,
        endpointURL: URL?,
        timeoutSeconds: Double
    ) async throws -> String {
        try await withThrowingTaskGroup(of: String.self) { group in
            group.addTask {
                try await service.transcribe(
                    samples16kMono: samples16kMono,
                    language: language,
                    endpointURL: endpointURL
                )
            }

            group.addTask {
                try await Task.sleep(nanoseconds: UInt64(timeoutSeconds * 1_000_000_000))
                throw TranscriptionError.backendUnavailable(
                    message: "\(service.backendName) timed out after \(Int(timeoutSeconds))s"
                )
            }

            guard let first = try await group.next() else {
                group.cancelAll()
                throw TranscriptionError.backendUnavailable(message: "Transcription failed to start")
            }
            group.cancelAll()
            return first
        }
    }

    private func formatSeconds(_ value: Double) -> String {
        let rounded = Int(max(0, value) * 10) / 10
        return "\(rounded)s"
    }
}
