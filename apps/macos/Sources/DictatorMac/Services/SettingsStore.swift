import Foundation

enum TranscriptionBackend: String, CaseIterable {
    case whisperKit = "whisperkit"
    case http       = "http"

    var displayName: String {
        switch self {
        case .whisperKit: return "WhisperKit (on-device)"
        case .http:       return "HTTP Server (Python)"
        }
    }
}

/// All user-configurable settings, persisted via UserDefaults.
final class SettingsStore {
    static let shared = SettingsStore()

    private let defaults = UserDefaults.standard

    // MARK: - Transcription

    var backend: TranscriptionBackend {
        get {
            let raw = defaults.string(forKey: "transcriptionBackend") ?? TranscriptionBackend.whisperKit.rawValue
            return TranscriptionBackend(rawValue: raw) ?? .whisperKit
        }
        set { defaults.set(newValue.rawValue, forKey: "transcriptionBackend") }
    }

    var language: String {
        get { defaults.string(forKey: "transcriptionLanguage") ?? "ru" }
        set { defaults.set(newValue, forKey: "transcriptionLanguage") }
    }

    // MARK: - WhisperKit

    var whisperKitModelName: String {
        get { defaults.string(forKey: "whisperKitModelName") ?? "openai_whisper-large-v3-turbo" }
        set { defaults.set(newValue, forKey: "whisperKitModelName") }
    }

    // MARK: - HTTP backend

    var httpEndpointURL: String {
        get { defaults.string(forKey: "httpEndpointURL") ?? "http://127.0.0.1:5500/transcribe" }
        set { defaults.set(newValue, forKey: "httpEndpointURL") }
    }

    // MARK: - Streaming

    var streamingEnabled: Bool {
        get { defaults.bool(forKey: "streamingEnabled") }
        set { defaults.set(newValue, forKey: "streamingEnabled") }
    }

    var chunkSeconds: Int {
        get {
            let v = defaults.integer(forKey: "chunkSeconds")
            return v > 0 ? v : 8
        }
        set { defaults.set(newValue, forKey: "chunkSeconds") }
    }

    // MARK: - Hotkey mode

    var hotkeyModeRaw: String {
        get { defaults.string(forKey: "hotkeyMode") ?? "smart" }
        set { defaults.set(newValue, forKey: "hotkeyMode") }
    }

    // MARK: - LLM (Ollama) correction

    var llmEnabled: Bool {
        get { defaults.bool(forKey: "llmEnabled") }
        set { defaults.set(newValue, forKey: "llmEnabled") }
    }

    var llmEndpointURL: String {
        get { defaults.string(forKey: "llmEndpointURL") ?? "http://127.0.0.1:11434" }
        set { defaults.set(newValue, forKey: "llmEndpointURL") }
    }

    var llmModel: String {
        get { defaults.string(forKey: "llmModel") ?? "llama3" }
        set { defaults.set(newValue, forKey: "llmModel") }
    }

    var llmSystemPrompt: String {
        get { defaults.string(forKey: "llmSystemPrompt") ?? LLMService.defaultCorrectionPrompt }
        set { defaults.set(newValue, forKey: "llmSystemPrompt") }
    }
}
