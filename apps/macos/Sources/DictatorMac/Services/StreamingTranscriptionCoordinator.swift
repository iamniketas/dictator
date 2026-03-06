import Foundation

@MainActor
final class StreamingTranscriptionCoordinator {
    enum Event {
        case loopStateChanged(Bool)
        case chunkStarted
        case chunkCompleted(chunksProcessed: Int, audioSeconds: Double, accumulatedText: String)
        case chunkFailed(String)
    }

    enum Finalization {
        case text(String)
        case needsFullFallback
    }

    private var loopTask: Task<Void, Never>?
    private var processedNativeIndex = 0
    private var accumulatedTranscript = ""
    private var isChunkTranscribing = false
    private var chunksProcessed = 0

    func reset() {
        loopTask?.cancel()
        loopTask = nil
        processedNativeIndex = 0
        accumulatedTranscript = ""
        isChunkTranscribing = false
        chunksProcessed = 0
    }

    func startLoop(
        chunkSecondsProvider: @escaping () -> Int,
        isRecordingProvider: @escaping () -> Bool,
        snapshotProvider: @escaping (_ fromIndex: Int) -> (samples: [Float], nextIndex: Int, sampleRate: Double),
        transcribeChunk: @escaping (_ samples16k: [Float]) async throws -> String,
        onEvent: @escaping (Event) -> Void
    ) {
        loopTask?.cancel()
        onEvent(.loopStateChanged(true))

        loopTask = Task { [weak self] in
            guard let self else { return }
            while !Task.isCancelled {
                if !isRecordingProvider() {
                    break
                }

                let chunkTarget = Double(max(chunkSecondsProvider(), 1))
                if self.unprocessedNativeDurationSeconds(snapshotProvider: snapshotProvider) >= chunkTarget {
                    await self.transcribeNextChunk(
                        isFinal: false,
                        snapshotProvider: snapshotProvider,
                        transcribeChunk: transcribeChunk,
                        onEvent: onEvent
                    )
                    continue
                }

                try? await Task.sleep(for: .milliseconds(350))
            }
            onEvent(.loopStateChanged(false))
        }
    }

    func stopLoopAndFinalize(
        snapshotProvider: @escaping (_ fromIndex: Int) -> (samples: [Float], nextIndex: Int, sampleRate: Double),
        transcribeChunk: @escaping (_ samples16k: [Float]) async throws -> String,
        onEvent: @escaping (Event) -> Void
    ) async -> Finalization {
        loopTask?.cancel()
        loopTask = nil

        let waitStartedAt = Date()
        while isChunkTranscribing {
            if Date().timeIntervalSince(waitStartedAt) > 15 {
                isChunkTranscribing = false
                break
            }
            try? await Task.sleep(for: .milliseconds(100))
        }

        await transcribeNextChunk(
            isFinal: true,
            snapshotProvider: snapshotProvider,
            transcribeChunk: transcribeChunk,
            onEvent: onEvent
        )

        let trimmed = accumulatedTranscript.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.isEmpty {
            return .needsFullFallback
        }
        return .text(trimmed)
    }

    private func unprocessedNativeDurationSeconds(
        snapshotProvider: (_ fromIndex: Int) -> (samples: [Float], nextIndex: Int, sampleRate: Double)
    ) -> Double {
        let snapshot = snapshotProvider(processedNativeIndex)
        guard snapshot.sampleRate > 0 else {
            return 0
        }
        return Double(snapshot.samples.count) / snapshot.sampleRate
    }

    private func transcribeNextChunk(
        isFinal: Bool,
        snapshotProvider: @escaping (_ fromIndex: Int) -> (samples: [Float], nextIndex: Int, sampleRate: Double),
        transcribeChunk: @escaping (_ samples16k: [Float]) async throws -> String,
        onEvent: @escaping (Event) -> Void
    ) async {
        if isChunkTranscribing {
            return
        }

        let snapshot = snapshotProvider(processedNativeIndex)
        processedNativeIndex = snapshot.nextIndex

        if snapshot.samples.isEmpty {
            return
        }

        let chunk16k = AudioCaptureService.resampleTo16k(samples: snapshot.samples, nativeSampleRate: snapshot.sampleRate)
        let audioSeconds = Double(chunk16k.count) / 16_000.0
        if !isFinal && audioSeconds < 0.2 {
            return
        }

        isChunkTranscribing = true
        onEvent(.chunkStarted)

        do {
            let text = try await transcribeChunk(chunk16k)
            chunksProcessed += 1

            if !text.isEmpty {
                if !accumulatedTranscript.isEmpty {
                    accumulatedTranscript.append(" ")
                }
                accumulatedTranscript.append(text)
            }

            onEvent(
                .chunkCompleted(
                    chunksProcessed: chunksProcessed,
                    audioSeconds: audioSeconds,
                    accumulatedText: accumulatedTranscript
                )
            )
        } catch {
            onEvent(.chunkFailed(error.localizedDescription))
        }

        isChunkTranscribing = false
    }
}
