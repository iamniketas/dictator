import Foundation

/// Errors produced by the Ollama LLM client.
enum LLMError: LocalizedError {
    case invalidURL(String)
    case badResponse(Int)
    case emptyResponse

    var errorDescription: String? {
        switch self {
        case .invalidURL(let url):   return "Invalid Ollama URL: \(url)"
        case .badResponse(let code): return "Ollama returned HTTP \(code)"
        case .emptyResponse:         return "Ollama returned an empty response"
        }
    }
}

/// Lightweight Ollama client.
///
/// Calls `POST /api/generate` with `stream: false`.
/// Compatible with Ollama ≥ 0.1.x running locally on any port.
struct LLMService {

    let endpointURL: String
    let model: String

    /// Sends `prompt` to Ollama and returns the generated text.
    func generate(prompt: String) async throws -> String {
        guard let url = URL(string: endpointURL.hasSuffix("/api/generate")
                            ? endpointURL
                            : endpointURL.trimmingCharacters(in: .init(charactersIn: "/")) + "/api/generate")
        else {
            throw LLMError.invalidURL(endpointURL)
        }

        let body: [String: Any] = [
            "model": model,
            "prompt": prompt,
            "stream": false
        ]
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONSerialization.data(withJSONObject: body)
        request.timeoutInterval = 120

        let (data, response) = try await URLSession.shared.data(for: request)

        if let http = response as? HTTPURLResponse, http.statusCode != 200 {
            throw LLMError.badResponse(http.statusCode)
        }

        guard let json = try JSONSerialization.jsonObject(with: data) as? [String: Any],
              let text = json["response"] as? String else {
            throw LLMError.emptyResponse
        }

        return text.trimmingCharacters(in: .whitespacesAndNewlines)
    }
}

// MARK: - Default correction prompt

extension LLMService {
    static let defaultCorrectionPrompt = """
        You are a speech-to-text post-processor. \
        Fix grammar, punctuation, and spelling in the following transcription. \
        Keep the original language and meaning. \
        Return only the corrected text — no explanations, no quotes.
        """

    /// Builds a correction prompt for the given raw transcript.
    static func correctionPrompt(for transcript: String, systemPrompt: String) -> String {
        "\(systemPrompt)\n\nText to correct:\n\(transcript)"
    }
}
