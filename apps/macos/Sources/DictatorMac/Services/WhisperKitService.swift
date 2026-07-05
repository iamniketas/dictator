import Foundation
import WhisperKit

// MARK: - Available models

struct WhisperKitModelInfo: Identifiable, Hashable {
    let id: String      // e.g. "openai_whisper-large-v3-turbo"
    let displayName: String
    let approxSizeMB: Int

    /// Full catalog of WhisperKit CoreML models available from argmaxinc/whisperkit-coreml.
    /// Sorted by approximate size ascending (smallest = fastest).
    static let available: [WhisperKitModelInfo] = [
        // ── Tiny/Base: ultra-fast, works on any Apple Silicon ──────────────────
        .init(id: "openai_whisper-tiny",                displayName: "tiny",                  approxSizeMB: 75),
        .init(id: "openai_whisper-tiny.en",             displayName: "tiny.en (English)",      approxSizeMB: 75),
        .init(id: "openai_whisper-base",                displayName: "base",                  approxSizeMB: 145),
        .init(id: "openai_whisper-base.en",             displayName: "base.en (English)",      approxSizeMB: 145),
        // ── Small/Medium: daily-driver tier ────────────────────────────────────
        .init(id: "openai_whisper-small",               displayName: "small",                 approxSizeMB: 480),
        .init(id: "openai_whisper-small.en",            displayName: "small.en (English)",     approxSizeMB: 480),
        .init(id: "openai_whisper-medium",              displayName: "medium",                approxSizeMB: 1_500),
        .init(id: "openai_whisper-medium.en",           displayName: "medium.en (English)",    approxSizeMB: 1_500),
        // ── Distil-Whisper: 2× faster large-v3 with minimal accuracy loss ──────
        .init(id: "distil-whisper_distil-large-v3",     displayName: "distil-large-v3 ⚡",    approxSizeMB: 800),
        // ── Large: maximum accuracy ─────────────────────────────────────────────
        .init(id: "openai_whisper-large-v3-turbo",      displayName: "large-v3-turbo ✦",      approxSizeMB: 1_600),
        .init(id: "openai_whisper-large-v3",            displayName: "large-v3",              approxSizeMB: 3_100),
        .init(id: "openai_whisper-large-v2",            displayName: "large-v2",              approxSizeMB: 3_000),
    ]
}

// MARK: - WhisperKit transcription service

@MainActor
final class WhisperKitTranscriptionService: TranscriptionService, ObservableObject {

    enum LoadState: Equatable {
        case idle
        /// Model files are being fetched from HuggingFace. Progress is indeterminate
        /// because WhisperKit's initializer does not expose download callbacks.
        case downloading
        /// Files are local; model is being loaded into memory / compiled by CoreML.
        case loading
        case ready(modelName: String)
        case failed(String)

        var isReady: Bool {
            if case .ready = self { return true }
            return false
        }

        var statusText: String {
            switch self {
            case .idle:                return "Not loaded"
            case .downloading:        return "Downloading from HuggingFace…"
            case .loading:            return "Loading model into memory…"
            case .ready(let name):    return "Ready (\(name))"
            case .failed(let msg):    return "Error: \(msg)"
            }
        }
    }

    @Published private(set) var loadState: LoadState = .idle

    private var kit: WhisperKit?

    // MARK: - Model management

    /// Load (and download if needed) the given model.
    func loadModel(_ modelName: String) async {
        guard loadState != .loading, loadState != .downloading else { return }

        // Unload previous instance
        kit = nil
        // Show the correct initial state depending on whether files are already cached.
        loadState = Self.isModelCached(modelName) ? .loading : .downloading

        do {
            let config = WhisperKitConfig(
                model: modelName,
                verbose: false,
                logLevel: .none
            )
            // WhisperKit transparently downloads missing files then loads the model.
            // We can't get granular progress from its initializer, so we stay in the
            // appropriate state until it completes.
            let instance = try await WhisperKit(config)
            kit = instance
            loadState = .ready(modelName: modelName)
        } catch {
            loadState = .failed(error.localizedDescription)
        }
    }

    // MARK: - Cache detection

    /// Returns true if all model files for `modelName` are present in the local HuggingFace cache.
    private static func isModelCached(_ modelName: String) -> Bool {
        guard let appSupport = FileManager.default
            .urls(for: .applicationSupportDirectory, in: .userDomainMask).first else { return false }
        // WhisperKit stores CoreML models under the argmaxinc/whisperkit-coreml organisation folder.
        let modelDir = appSupport
            .appendingPathComponent("huggingface/models/argmaxinc/whisperkit-coreml")
            .appendingPathComponent(modelName)
        return FileManager.default.fileExists(atPath: modelDir.path)
    }

    func unload() {
        kit = nil
        loadState = .idle
    }

    // MARK: - TranscriptionService

    func transcribe(samples16kMono: [Float], language: String) async throws -> String {
        guard let kit, case .ready = loadState else {
            throw TranscriptionError.modelNotLoaded
        }

        var options = DecodingOptions()
        options.language = language.isEmpty ? nil : language
        options.task = .transcribe

        let results = try await kit.transcribe(audioArray: samples16kMono, decodeOptions: options)
        let text = results
            .map(\.text)
            .joined(separator: " ")
            .split(separator: " ")
            .joined(separator: " ")   // collapse multiple spaces
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return text
    }
}
