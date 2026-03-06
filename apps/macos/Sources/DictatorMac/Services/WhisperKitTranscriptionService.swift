import Foundation

#if canImport(WhisperKit)
import WhisperKit

actor WhisperKitRuntime {
    private var instance: WhisperKit?
    private let modelFolder: URL

    init(modelFolder: URL) {
        self.modelFolder = modelFolder
    }

    func transcribe(samples16kMono: [Float], language: String) async throws -> String {
        let kit = try await resolveInstance()
        let options = DecodingOptions(
            verbose: false,
            task: .transcribe,
            language: language,
            temperatureFallbackCount: 0,
            chunkingStrategy: nil
        )

        let results = try await kit.transcribe(audioArray: samples16kMono, decodeOptions: options)
        let joined = results
            .map { $0.text.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
            .joined(separator: " ")
            .trimmingCharacters(in: .whitespacesAndNewlines)

        guard !joined.isEmpty else {
            throw TranscriptionError.invalidPayload
        }

        return joined
    }

    private func resolveInstance() async throws -> WhisperKit {
        if let instance {
            return instance
        }

        let config = WhisperKitConfig(
            modelFolder: modelFolder.path,
            verbose: false,
            logLevel: .none,
            prewarm: false,
            load: true,
            download: false
        )

        let created = try await WhisperKit(config)
        instance = created
        return created
    }
}

final class WhisperKitTranscriptionService: TranscriptionService {
    let backendName = "whisperkit"

    private let runtime: WhisperKitRuntime?

    init() {
        if let modelFolder = Self.resolveModelFolder() {
            runtime = WhisperKitRuntime(modelFolder: modelFolder)
        } else {
            runtime = nil
        }
    }

    var isAvailable: Bool {
        guard runtime != nil else {
            return false
        }
#if arch(arm64)
        return true
#else
        return false
#endif
    }

    func transcribe(samples16kMono: [Float], language: String, endpointURL: URL?) async throws -> String {
        guard let runtime else {
            throw TranscriptionError.backendUnavailable(message: "WhisperKit model folder not found")
        }
        return try await runtime.transcribe(samples16kMono: samples16kMono, language: language)
    }

    private static func resolveModelFolder() -> URL? {
        let fileManager = FileManager.default

        if let env = ProcessInfo.processInfo.environment["WHISPERKIT_MODEL_DIR"], !env.isEmpty {
            let envURL = URL(fileURLWithPath: env, isDirectory: true)
            if hasRequiredModelFiles(in: envURL) {
                return envURL
            }
        }

        guard let appSupport = fileManager.urls(for: .applicationSupportDirectory, in: .userDomainMask).first else {
            return nil
        }

        let base = appSupport
            .appendingPathComponent("Dictator", isDirectory: true)
            .appendingPathComponent("WhisperKitModels", isDirectory: true)

        if hasRequiredModelFiles(in: base) {
            return base
        }

        guard let children = try? fileManager.contentsOfDirectory(
            at: base,
            includingPropertiesForKeys: [.isDirectoryKey],
            options: [.skipsHiddenFiles]
        ) else {
            return nil
        }

        for child in children {
            if hasRequiredModelFiles(in: child) {
                return child
            }
        }

        return nil
    }

    private static func hasRequiredModelFiles(in directory: URL) -> Bool {
        guard let enumerator = FileManager.default.enumerator(
            at: directory,
            includingPropertiesForKeys: [.isRegularFileKey],
            options: [.skipsHiddenFiles]
        ) else {
            return false
        }

        var hasMel = false
        var hasEncoder = false
        var hasDecoder = false

        for case let url as URL in enumerator {
            let name = url.lastPathComponent.lowercased()
            if name.contains("melspectrogram") {
                hasMel = true
            }
            if name.contains("audioencoder") {
                hasEncoder = true
            }
            if name.contains("textdecoder") {
                hasDecoder = true
            }
            if hasMel && hasEncoder && hasDecoder {
                return true
            }
        }

        return false
    }
}

#else

final class WhisperKitTranscriptionService: TranscriptionService {
    let backendName = "whisperkit"
    let isAvailable = false

    func transcribe(samples16kMono: [Float], language: String, endpointURL: URL?) async throws -> String {
        throw TranscriptionError.backendUnavailable(message: "WhisperKit module is not available in this build")
    }
}

#endif
