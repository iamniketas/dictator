import Foundation

// MARK: - Shared runtime paths (mirrors Contora's SharedRuntimePaths.swift)

/// Canonical paths for the NiketasAI shared runtime used by both Contora and Dictator.
///
/// Layout on disk:
/// ```
/// ~/Library/Application Support/NiketasAI/runtime/
///   transcription-server.json   ← active backend config (written by Contora)
///   faster-whisper-xxl/         ← legacy faster-whisper executable + models
///   mlx-server/                 ← MLX server toolkit (managed by Contora)
/// ```
enum SharedRuntimePaths {
    static func runtimeRoot() -> URL {
        if let override = ProcessInfo.processInfo.environment["NIKETAS_SHARED_RUNTIME_ROOT"],
           !override.isEmpty {
            return URL(fileURLWithPath: override, isDirectory: true)
        }
        let fm = FileManager.default
        if let appSupport = fm.urls(for: .applicationSupportDirectory, in: .userDomainMask).first {
            return appSupport.appendingPathComponent("NiketasAI/runtime", isDirectory: true)
        }
        return URL(fileURLWithPath: NSHomeDirectory())
            .appendingPathComponent("Library/Application Support/NiketasAI/runtime", isDirectory: true)
    }

    static func transcriptionServerConfigURL() -> URL {
        runtimeRoot().appendingPathComponent("transcription-server.json")
    }
}

// MARK: - Shared transcription server config

/// Mirrors Contora's `SharedTranscriptionServerConfig`.
/// Written by Contora, read by Dictator to discover the active backend and MLX endpoint.
struct SharedTranscriptionServerConfig: Codable {
    var schemaVersion: String
    var activeBackend: ContoraMacBackend
    var whisperTranscribeURL: String
    var mlxTranscribeURL: String
    var mlxModelID: String
    var updatedAt: String

    /// The backend key used by Contora.
    enum ContoraMacBackend: String, Codable {
        case whisperHTTP    = "whisper_http"
        case mlxOpenAIHTTP  = "mlx_openai_http"
    }

    static func load() -> SharedTranscriptionServerConfig? {
        let url = SharedRuntimePaths.transcriptionServerConfigURL()
        guard FileManager.default.fileExists(atPath: url.path),
              let data = try? Data(contentsOf: url),
              let config = try? JSONDecoder().decode(Self.self, from: data) else {
            return nil
        }
        return config
    }

    /// Default values matching Contora's default config.
    static func defaultConfig() -> SharedTranscriptionServerConfig {
        SharedTranscriptionServerConfig(
            schemaVersion: "1.0",
            activeBackend: .mlxOpenAIHTTP,
            whisperTranscribeURL: "http://127.0.0.1:5500/transcribe",
            mlxTranscribeURL: "http://127.0.0.1:8000/v1/audio/transcriptions",
            mlxModelID: "mlx-community/whisper-large-v3-turbo-asr-fp16",
            updatedAt: ISO8601DateFormatter().string(from: Date())
        )
    }
}

// MARK: - Server probe

extension SharedTranscriptionServerConfig {
    /// Checks whether the MLX server is reachable by hitting GET /v1/models.
    static func probeMLC(url: String) async -> Bool {
        guard let transcribeURL = URL(string: url),
              var components = URLComponents(url: transcribeURL, resolvingAgainstBaseURL: false) else {
            return false
        }
        components.path = "/v1/models"
        guard let modelsURL = components.url else { return false }

        var request = URLRequest(url: modelsURL)
        request.timeoutInterval = 3
        do {
            let (_, response) = try await URLSession.shared.data(for: request)
            return (response as? HTTPURLResponse).map { (200...299).contains($0.statusCode) } ?? false
        } catch {
            return false
        }
    }

    /// Checks whether the legacy Whisper HTTP server is reachable via GET /health.
    static func probeWhisper(url: String) async -> Bool {
        guard let transcribeURL = URL(string: url),
              var components = URLComponents(url: transcribeURL, resolvingAgainstBaseURL: false) else {
            return false
        }
        components.path = "/health"
        guard let healthURL = components.url else { return false }

        var request = URLRequest(url: healthURL)
        request.timeoutInterval = 3
        do {
            let (_, response) = try await URLSession.shared.data(for: request)
            return (response as? HTTPURLResponse).map { (200...299).contains($0.statusCode) } ?? false
        } catch {
            return false
        }
    }
}
