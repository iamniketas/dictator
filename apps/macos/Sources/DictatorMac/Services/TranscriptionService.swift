import Foundation

// MARK: - Protocol

protocol TranscriptionService: AnyObject {
    /// Transcribe 16kHz mono float samples. Returns the recognised text.
    func transcribe(samples16kMono: [Float], language: String) async throws -> String
}

// MARK: - Errors

enum TranscriptionError: LocalizedError {
    case badResponse
    case serverError(statusCode: Int, message: String)
    case invalidPayload
    case modelNotLoaded
    case whisperKitError(String)

    var errorDescription: String? {
        switch self {
        case .badResponse:
            return "Invalid response from transcription server."
        case let .serverError(code, msg):
            return "Transcription server error \(code): \(msg)"
        case .invalidPayload:
            return "Transcription payload is missing text."
        case .modelNotLoaded:
            return "WhisperKit model is not loaded. Open Settings to download and load a model."
        case let .whisperKitError(msg):
            return "WhisperKit error: \(msg)"
        }
    }
}

// MARK: - HTTP implementation

final class WhisperHTTPTranscriptionService: TranscriptionService {
    func transcribe(samples16kMono: [Float], language: String) async throws -> String {
        let settings = SettingsStore.shared
        guard let endpointURL = URL(string: settings.httpEndpointURL) else {
            throw TranscriptionError.serverError(statusCode: 0, message: "Invalid endpoint URL: \(settings.httpEndpointURL)")
        }
        return try await transcribe(samples16kMono: samples16kMono, language: language, endpointURL: endpointURL)
    }

    func transcribe(samples16kMono: [Float], language: String, endpointURL: URL) async throws -> String {
        let wavData = WAVEncoder.makeWAVData(samples: samples16kMono, sampleRate: 16_000)
        let boundary = "Boundary-\(UUID().uuidString)"
        let body = makeMultipartBody(wavData: wavData, language: language, boundary: boundary)

        let audioSeconds = Double(samples16kMono.count) / 16_000.0
        let timeout = max(120, min(1_800, audioSeconds * 2.0 + 60))

        var request = URLRequest(url: endpointURL)
        request.httpMethod = "POST"
        request.timeoutInterval = timeout
        request.setValue("multipart/form-data; boundary=\(boundary)", forHTTPHeaderField: "Content-Type")
        request.httpBody = body

        let (data, response): (Data, URLResponse)
        do {
            (data, response) = try await URLSession.shared.data(for: request)
        } catch let error as URLError where error.code == .timedOut {
            throw TranscriptionError.serverError(
                statusCode: 408,
                message: "Client timeout after \(Int(timeout))s. Try streaming mode or a shorter recording."
            )
        }

        guard let http = response as? HTTPURLResponse else { throw TranscriptionError.badResponse }
        guard (200...299).contains(http.statusCode) else {
            let msg = String(data: data, encoding: .utf8) ?? "Unknown server error"
            throw TranscriptionError.serverError(statusCode: http.statusCode, message: msg)
        }
        guard
            let json = try JSONSerialization.jsonObject(with: data) as? [String: Any],
            let text = json["text"] as? String
        else { throw TranscriptionError.invalidPayload }

        return text.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    // MARK: - Multipart helper

    private func makeMultipartBody(wavData: Data, language: String, boundary: String) -> Data {
        var body = Data()
        let crlf = "\r\n"

        body.append("--\(boundary)\(crlf)".data(using: .utf8)!)
        body.append("Content-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\(crlf)".data(using: .utf8)!)
        body.append("Content-Type: audio/wav\(crlf)\(crlf)".data(using: .utf8)!)
        body.append(wavData)
        body.append(crlf.data(using: .utf8)!)

        body.append("--\(boundary)\(crlf)".data(using: .utf8)!)
        body.append("Content-Disposition: form-data; name=\"language\"\(crlf)\(crlf)".data(using: .utf8)!)
        body.append("\(language)\(crlf)".data(using: .utf8)!)

        body.append("--\(boundary)--\(crlf)".data(using: .utf8)!)
        return body
    }
}
