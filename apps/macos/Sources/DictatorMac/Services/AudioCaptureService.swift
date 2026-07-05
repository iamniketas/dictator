import AVFoundation

enum AudioCaptureError: LocalizedError {
    case alreadyRunning
    case notRunning

    var errorDescription: String? {
        switch self {
        case .alreadyRunning: return "Recording is already running."
        case .notRunning:     return "Recording is not running."
        }
    }
}

struct AudioCaptureResult {
    let samples16kMono: [Float]
    let durationSeconds: Double
    let nativeSamplesCount: Int
}

final class AudioCaptureService {
    private let engine = AVAudioEngine()
    private let lock = NSLock()

    private var nativeMonoSamples: [Float] = []
    private var nativeSampleRate: Double = 16_000
    private var isRunning = false
    private var startTime: Date?

    /// RMS amplitude of the most recently captured audio buffer (0…1).
    /// Thread-safe: updated under `lock`, safe to read from any thread.
    private var _currentRMS: Float = 0
    var currentRMS: Float {
        lock.lock()
        defer { lock.unlock() }
        return _currentRMS
    }

    // MARK: - Public API

    func startCapture() throws {
        lock.lock()
        defer { lock.unlock() }

        guard !isRunning else { throw AudioCaptureError.alreadyRunning }

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

        guard isRunning else { throw AudioCaptureError.notRunning }

        engine.inputNode.removeTap(onBus: 0)
        engine.stop()
        isRunning = false

        let native = nativeMonoSamples
        let sampleRate = nativeSampleRate
        nativeMonoSamples.removeAll(keepingCapacity: true)
        startTime = nil

        let downsampled = Self.resampleTo16k(samples: native, nativeSampleRate: sampleRate)
        let duration = Double(downsampled.count) / 16_000.0

        return AudioCaptureResult(
            samples16kMono: downsampled,
            durationSeconds: duration,
            nativeSamplesCount: native.count
        )
    }

    func elapsedSeconds() -> Double {
        lock.lock()
        defer { lock.unlock() }
        guard isRunning, let startTime else { return 0 }
        return Date().timeIntervalSince(startTime)
    }

    /// Snapshot of unprocessed native samples starting from `startIndex`.
    func snapshotNativeMono(from startIndex: Int) -> (samples: [Float], nextIndex: Int, sampleRate: Double) {
        lock.lock()
        let safeStart = min(max(0, startIndex), nativeMonoSamples.count)
        let slice = Array(nativeMonoSamples[safeStart...])
        let nextIndex = nativeMonoSamples.count
        let sampleRate = nativeSampleRate
        lock.unlock()
        return (slice, nextIndex, sampleRate)
    }

    // MARK: - Internal

    private func appendNativeMonoSamples(buffer: AVAudioPCMBuffer) {
        guard let data = buffer.floatChannelData else { return }
        let channels = Int(buffer.format.channelCount)
        let frames = Int(buffer.frameLength)
        guard frames > 0, channels > 0 else { return }

        var mono = [Float](repeating: 0, count: frames)
        if channels == 1 {
            let ch = data[0]
            for i in 0..<frames { mono[i] = ch[i] }
        } else {
            for frame in 0..<frames {
                var sum: Float = 0
                for ch in 0..<channels { sum += data[ch][frame] }
                mono[frame] = sum / Float(channels)
            }
        }

        // Compute RMS for this buffer so callers can poll live amplitude.
        var sumSq: Float = 0
        for s in mono { sumSq += s * s }
        let rms = mono.isEmpty ? 0 : sqrtf(sumSq / Float(mono.count))

        lock.lock()
        _currentRMS = rms
        nativeMonoSamples.append(contentsOf: mono)
        lock.unlock()
    }

    // MARK: - Resampling (linear interpolation)

    static func resampleTo16k(samples: [Float], nativeSampleRate: Double) -> [Float] {
        guard !samples.isEmpty else { return [] }
        if abs(nativeSampleRate - 16_000.0) < 0.01 { return samples }

        let ratio = nativeSampleRate / 16_000.0
        let outCount = max(1, Int(Double(samples.count) / ratio))
        var output = [Float]()
        output.reserveCapacity(outCount)

        for outIndex in 0..<outCount {
            let srcPos = Double(outIndex) * ratio
            let srcIndex = Int(srcPos)
            if srcIndex + 1 < samples.count {
                let frac = Float(srcPos - Double(srcIndex))
                output.append(samples[srcIndex] * (1 - frac) + samples[srcIndex + 1] * frac)
            } else if srcIndex < samples.count {
                output.append(samples[srcIndex])
            }
        }
        return output
    }
}
