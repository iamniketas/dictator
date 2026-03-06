import AVFoundation
import Foundation

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

final class AudioCaptureService {
    private let engine = AVAudioEngine()
    private let lock = NSLock()

    private var nativeMonoSamples: [Float] = []
    private var nativeSampleRate: Double = 16_000
    private var latestRMS: Double = 0
    private var smoothedBars: [Double] = []
    private var isRunning = false
    private var startTime: Date?

    func startCapture() throws {
        lock.lock()
        defer { lock.unlock() }

        guard !isRunning else {
            throw AudioCaptureError.alreadyRunning
        }

        nativeMonoSamples.removeAll(keepingCapacity: true)
        latestRMS = 0
        smoothedBars.removeAll(keepingCapacity: true)

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
        latestRMS = 0
        smoothedBars.removeAll(keepingCapacity: true)

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

    func currentInputLevel() -> Double {
        lock.lock()
        let level = latestRMS
        lock.unlock()
        return level
    }

    func currentWaveformBars(count: Int) -> [Double] {
        lock.lock()
        let totalCount = nativeMonoSamples.count
        let tailWindow = min(totalCount, 4096)
        let tailStart = totalCount - tailWindow
        let tail = tailWindow > 0 ? Array(nativeMonoSamples[tailStart..<totalCount]) : []
        let previous = smoothedBars
        lock.unlock()

        guard count > 0 else { return [] }
        guard !tail.isEmpty else {
            let zeros = Array(repeating: 0.0, count: count)
            lock.lock()
            smoothedBars = zeros
            lock.unlock()
            return zeros
        }

        let samplesPerBar = max(1, tail.count / count)
        var bars = Array(repeating: 0.0, count: count)
        let noiseFloor = 0.0035
        let loudSpeechRMS = 0.055

        for bar in 0..<count {
            let lo = bar * samplesPerBar
            let hi = min(tail.count, lo + samplesPerBar)
            if lo >= hi {
                continue
            }
            var sumSquares = 0.0
            for i in lo..<hi {
                let v = Double(tail[i])
                sumSquares += v * v
            }
            let rms = sqrt(sumSquares / Double(hi - lo))
            let normalized = (rms - noiseFloor) / max(0.0001, (loudSpeechRMS - noiseFloor))
            let clamped = min(max(normalized, 0), 1)
            bars[bar] = pow(clamped, 0.58)
        }

        var smoothed = Array(repeating: 0.0, count: count)
        for i in 0..<count {
            let old = i < previous.count ? previous[i] : 0
            let target = bars[i]
            let alpha = target >= old ? 0.74 : 0.36
            smoothed[i] = old + ((target - old) * alpha)
        }

        lock.lock()
        smoothedBars = smoothed
        lock.unlock()
        return smoothed
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
        if !mono.isEmpty {
            let sumSquares = mono.reduce(0.0) { partial, sample in
                partial + Double(sample * sample)
            }
            let rms = sqrt(sumSquares / Double(mono.count))
            latestRMS = min(max(rms, 0), 1)
        }
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
