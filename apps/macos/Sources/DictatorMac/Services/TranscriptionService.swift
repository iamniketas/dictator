import Foundation

enum TranscriptionBackendPreference: String, CaseIterable, Identifiable {
    case auto
    case whisperKit
    case http

    var id: String { rawValue }

    var title: String {
        switch self {
        case .auto:
            return "Automatic (WhisperKit, then HTTP)"
        case .whisperKit:
            return "WhisperKit (CoreML)"
        case .http:
            return "HTTP Server Fallback"
        }
    }
}

enum TranscriptionError: LocalizedError {
    case badResponse
    case serverError(statusCode: Int, message: String)
    case invalidPayload
    case invalidEndpoint
    case backendUnavailable(message: String)

    var errorDescription: String? {
        switch self {
        case .badResponse:
            return "Invalid response from transcription server."
        case let .serverError(statusCode, message):
            return "Transcription server error \(statusCode): \(message)"
        case .invalidPayload:
            return "Transcription payload is missing text."
        case .invalidEndpoint:
            return "Invalid transcription endpoint URL."
        case let .backendUnavailable(message):
            return "Transcription backend unavailable: \(message)"
        }
    }
}

protocol TranscriptionService {
    var backendName: String { get }
    var isAvailable: Bool { get }
    func transcribe(samples16kMono: [Float], language: String, endpointURL: URL?) async throws -> String
}

final class WhisperHTTPTranscriptionService: TranscriptionService {
    let backendName = "http"
    var isAvailable: Bool { true }

    func transcribe(samples16kMono: [Float], language: String, endpointURL: URL?) async throws -> String {
        guard let endpointURL else {
            throw TranscriptionError.invalidEndpoint
        }
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
