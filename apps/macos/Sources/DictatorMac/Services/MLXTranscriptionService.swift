import Foundation

/// Sends audio to an MLX Whisper server that exposes an OpenAI-compatible transcription API.
///
/// Compatible with:
/// - Contora's MLX server (`mlx-community/whisper-large-v3-turbo-asr-fp16` and others)
/// - Any server implementing `POST /v1/audio/transcriptions` (OpenAI Whisper format)
///
/// Request format (multipart/form-data):
///   - `file`:            audio/wav data
///   - `model`:           HuggingFace model ID (e.g. "mlx-community/whisper-large-v3-turbo-asr-fp16")
///   - `language`:        ISO 639-1 code (optional, "" = auto-detect)
///   - `response_format`: "json"
///
/// Response: `{"text": "transcribed text"}`
final class MLXTranscriptionService: TranscriptionService {

    let endpointURL: String
    let modelID: String

    init(endpointURL: String, modelID: String) {
        self.endpointURL = endpointURL
        self.modelID = modelID
    }

    func transcribe(samples16kMono: [Float], language: String) async throws -> String {
        guard let url = URL(string: endpointURL) else {
            throw TranscriptionError.serverError(statusCode: 0, message: "Invalid MLX endpoint URL: \(endpointURL)")
        }

        let wavData = buildWAV(samples: samples16kMono, sampleRate: 16_000)
        let audioSeconds = Double(samples16kMono.count) / 16_000.0
        let timeoutSeconds = max(30, min(300, audioSeconds * 1.5 + 10))

        let boundary = "Boundary-\(UUID().uuidString)"
        var body = Data()

        // --- file field ---
        body.appendMultipartField(
            boundary: boundary, name: "file", filename: "audio.wav",
            contentType: "audio/wav", data: wavData
        )
        // --- model field ---
        body.appendMultipartString(boundary: boundary, name: "model", value: modelID)
        // --- language field ---
        if !language.isEmpty {
            body.appendMultipartString(boundary: boundary, name: "language", value: language)
        }
        // --- response_format ---
        body.appendMultipartString(boundary: boundary, name: "response_format", value: "json")
        // --- closing boundary ---
        body.append("--\(boundary)--\r\n".data(using: .utf8)!)

        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.setValue("multipart/form-data; boundary=\(boundary)", forHTTPHeaderField: "Content-Type")
        request.httpBody = body
        request.timeoutInterval = timeoutSeconds

        let (data, response) = try await URLSession.shared.data(for: request)

        if let http = response as? HTTPURLResponse, http.statusCode != 200 {
            let body = String(data: data, encoding: .utf8) ?? ""
            throw TranscriptionError.serverError(statusCode: http.statusCode, message: "MLX: \(body)")
        }

        guard let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let text = json["text"] as? String else {
            throw TranscriptionError.serverError(statusCode: 0, message: "MLX returned unexpected response format")
        }

        return text
            .split(separator: " ").joined(separator: " ")   // collapse extra spaces
            .trimmingCharacters(in: .whitespacesAndNewlines)
    }

    // MARK: - WAV builder

    private func buildWAV(samples: [Float], sampleRate: Int) -> Data {
        let numSamples = samples.count
        let byteRate = sampleRate * 2          // 16-bit mono
        let dataSize = numSamples * 2
        let chunkSize = 36 + dataSize

        var wav = Data()
        wav.appendString("RIFF")
        wav.appendLE32(UInt32(chunkSize))
        wav.appendString("WAVE")
        wav.appendString("fmt ")
        wav.appendLE32(16)                      // PCM chunk size
        wav.appendLE16(1)                       // PCM format
        wav.appendLE16(1)                       // mono
        wav.appendLE32(UInt32(sampleRate))
        wav.appendLE32(UInt32(byteRate))
        wav.appendLE16(2)                       // block align
        wav.appendLE16(16)                      // bits per sample
        wav.appendString("data")
        wav.appendLE32(UInt32(dataSize))

        for s in samples {
            let clamped = max(-1.0, min(1.0, s))
            let int16 = Int16(clamped * 32767.0)
            wav.appendLE16(UInt16(bitPattern: int16))
        }
        return wav
    }
}

// MARK: - MLX-specific model catalog

struct MLXModelInfo: Identifiable, Hashable {
    let id: String          // full HuggingFace model ID
    let displayName: String
    let approxSizeMB: Int
    let note: String

    static let recommended: [MLXModelInfo] = [
        .init(id: "mlx-community/whisper-large-v3-turbo-asr-fp16",
              displayName: "large-v3-turbo fp16 ✦",
              approxSizeMB: 1_600,
              note: "Best quality/speed on Apple Silicon. Default in Contora."),
        .init(id: "mlx-community/distil-whisper-large-v3",
              displayName: "distil-large-v3",
              approxSizeMB: 800,
              note: "~2× faster than large-v3, small accuracy tradeoff."),
        .init(id: "mlx-community/whisper-large-v3-mlx-fp16",
              displayName: "large-v3 fp16",
              approxSizeMB: 3_100,
              note: "Maximum accuracy. Slower first inference (CoreML compile)."),
        .init(id: "mlx-community/whisper-small-mlx-q4",
              displayName: "small q4",
              approxSizeMB: 80,
              note: "Ultra-fast. Good for quick notes on older M1 devices."),
    ]
}

// MARK: - Data helpers

private extension Data {
    mutating func appendString(_ s: String) {
        if let d = s.data(using: .ascii) { append(d) }
    }
    mutating func appendLE16(_ v: UInt16) {
        var le = v.littleEndian; append(Data(bytes: &le, count: 2))
    }
    mutating func appendLE32(_ v: UInt32) {
        var le = v.littleEndian; append(Data(bytes: &le, count: 4))
    }
    mutating func appendMultipartField(boundary: String, name: String, filename: String,
                                       contentType: String, data: Data) {
        appendString("--\(boundary)\r\n")
        appendString("Content-Disposition: form-data; name=\"\(name)\"; filename=\"\(filename)\"\r\n")
        appendString("Content-Type: \(contentType)\r\n\r\n")
        append(data)
        appendString("\r\n")
    }
    mutating func appendMultipartString(boundary: String, name: String, value: String) {
        appendString("--\(boundary)\r\n")
        appendString("Content-Disposition: form-data; name=\"\(name)\"\r\n\r\n")
        appendString(value)
        appendString("\r\n")
    }
}
