import Foundation

final class SettingsStore {
    private enum Key {
        static let streamingEnabled = "dictator.streamingEnabled"
        static let launchAtLogin = "dictator.launchAtLogin"
        static let chunkSeconds = "dictator.chunkSeconds"
        static let transcriptionBackendPreference = "dictator.transcriptionBackendPreference"
        static let textInjectionMode = "dictator.textInjectionMode"
        static let transcriptionLanguage = "dictator.transcriptionLanguage"
        static let transcriptionEndpoint = "dictator.transcriptionEndpoint"
        static let recordingRetentionPolicy = "dictator.recordingRetentionPolicy"
    }

    private let defaults: UserDefaults

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
    }

    func loadStreamingEnabled(default defaultValue: Bool) -> Bool {
        readBool(Key.streamingEnabled) ?? defaultValue
    }

    func saveStreamingEnabled(_ value: Bool) {
        defaults.set(value, forKey: Key.streamingEnabled)
    }

    func loadLaunchAtLogin(default defaultValue: Bool) -> Bool {
        readBool(Key.launchAtLogin) ?? defaultValue
    }

    func saveLaunchAtLogin(_ value: Bool) {
        defaults.set(value, forKey: Key.launchAtLogin)
    }

    func loadChunkSeconds(default defaultValue: Int) -> Int {
        readInt(Key.chunkSeconds) ?? defaultValue
    }

    func saveChunkSeconds(_ value: Int) {
        defaults.set(value, forKey: Key.chunkSeconds)
    }

    func loadTranscriptionBackendPreference(default defaultValue: TranscriptionBackendPreference) -> TranscriptionBackendPreference {
        guard
            let raw = readString(Key.transcriptionBackendPreference),
            let value = TranscriptionBackendPreference(rawValue: raw)
        else {
            return defaultValue
        }
        return value
    }

    func saveTranscriptionBackendPreference(_ value: TranscriptionBackendPreference) {
        defaults.set(value.rawValue, forKey: Key.transcriptionBackendPreference)
    }

    func loadTextInjectionMode(default defaultValue: TextInjectionMode) -> TextInjectionMode {
        guard
            let raw = readString(Key.textInjectionMode),
            let value = TextInjectionMode(rawValue: raw)
        else {
            return defaultValue
        }
        return value
    }

    func saveTextInjectionMode(_ value: TextInjectionMode) {
        defaults.set(value.rawValue, forKey: Key.textInjectionMode)
    }

    func loadTranscriptionLanguage(default defaultValue: String) -> String {
        readString(Key.transcriptionLanguage) ?? defaultValue
    }

    func saveTranscriptionLanguage(_ value: String) {
        defaults.set(value, forKey: Key.transcriptionLanguage)
    }

    func loadTranscriptionEndpoint(default defaultValue: String) -> String {
        readString(Key.transcriptionEndpoint) ?? defaultValue
    }

    func saveTranscriptionEndpoint(_ value: String) {
        defaults.set(value, forKey: Key.transcriptionEndpoint)
    }

    func loadRecordingRetentionPolicy(default defaultValue: RecordingRetentionPolicy) -> RecordingRetentionPolicy {
        guard
            let raw = readString(Key.recordingRetentionPolicy),
            let value = RecordingRetentionPolicy(rawValue: raw)
        else {
            return defaultValue
        }
        return value
    }

    func saveRecordingRetentionPolicy(_ value: RecordingRetentionPolicy) {
        defaults.set(value.rawValue, forKey: Key.recordingRetentionPolicy)
    }

    private func readBool(_ key: String) -> Bool? {
        guard defaults.object(forKey: key) != nil else {
            return nil
        }
        return defaults.bool(forKey: key)
    }

    private func readInt(_ key: String) -> Int? {
        guard defaults.object(forKey: key) != nil else {
            return nil
        }
        return defaults.integer(forKey: key)
    }

    private func readString(_ key: String) -> String? {
        defaults.string(forKey: key)
    }
}
