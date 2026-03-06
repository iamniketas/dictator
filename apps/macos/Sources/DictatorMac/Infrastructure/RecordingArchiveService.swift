import Foundation

enum RecordingArchiveError: LocalizedError {
    case appSupportDirectoryUnavailable

    var errorDescription: String? {
        switch self {
        case .appSupportDirectoryUnavailable:
            return "Application Support directory is unavailable."
        }
    }
}

enum RecordingRetentionPolicy: String, CaseIterable, Identifiable {
    case keepAll
    case keepLast5
    case keepLast3Days
    case keepLast5Days

    var id: String { rawValue }

    var title: String {
        switch self {
        case .keepAll:
            return "Keep Everything"
        case .keepLast5:
            return "Keep Last 5 Recordings"
        case .keepLast3Days:
            return "Keep Last 3 Days"
        case .keepLast5Days:
            return "Keep Last 5 Days"
        }
    }
}

enum WAVEncoder {
    static func makeWAVData(samples: [Float], sampleRate: UInt32) -> Data {
        let channelCount: UInt16 = 1
        let bitsPerSample: UInt16 = 32
        let byteRate = sampleRate * UInt32(channelCount) * UInt32(bitsPerSample / 8)
        let blockAlign = channelCount * (bitsPerSample / 8)
        let dataSize = UInt32(samples.count * MemoryLayout<Float>.size)
        let riffSize = UInt32(36) + dataSize

        var data = Data()
        data.reserveCapacity(Int(44 + dataSize))

        data.append("RIFF".data(using: .ascii)!)
        data.appendUInt32LE(riffSize)
        data.append("WAVE".data(using: .ascii)!)

        data.append("fmt ".data(using: .ascii)!)
        data.appendUInt32LE(16)
        data.appendUInt16LE(3)
        data.appendUInt16LE(channelCount)
        data.appendUInt32LE(sampleRate)
        data.appendUInt32LE(byteRate)
        data.appendUInt16LE(blockAlign)
        data.appendUInt16LE(bitsPerSample)

        data.append("data".data(using: .ascii)!)
        data.appendUInt32LE(dataSize)
        for value in samples {
            data.appendFloat32LE(value)
        }

        return data
    }
}

final class RecordingArchiveService {
    private let fileManager = FileManager.default

    func saveRecording(samples16kMono: [Float], sampleRate: UInt32 = 16_000) throws -> URL {
        let recordingsDirectory = try makeRecordingsDirectory()
        let filename = "recording-\(timestampString())-\(UUID().uuidString.prefix(8)).wav"
        let fileURL = recordingsDirectory.appendingPathComponent(filename)

        let wavData = WAVEncoder.makeWAVData(samples: samples16kMono, sampleRate: sampleRate)
        try wavData.write(to: fileURL, options: .atomic)
        return fileURL
    }

    func recordingsDirectoryURL() throws -> URL {
        try makeRecordingsDirectory()
    }

    func archiveStats() throws -> (directory: URL, recordingsCount: Int, totalBytes: Int64) {
        let directory = try makeRecordingsDirectory()
        let fileURLs = try fileManager.contentsOfDirectory(
            at: directory,
            includingPropertiesForKeys: [.fileSizeKey],
            options: [.skipsHiddenFiles]
        )

        var count = 0
        var bytes: Int64 = 0
        for url in fileURLs where url.pathExtension.lowercased() == "wav" {
            count += 1
            let size = try url.resourceValues(forKeys: [.fileSizeKey]).fileSize ?? 0
            bytes += Int64(size)
        }

        return (directory, count, bytes)
    }

    func applyRetention(_ policy: RecordingRetentionPolicy) throws {
        let directory = try makeRecordingsDirectory()
        try pruneRecordings(in: directory, policy: policy)
    }

    func saveTranscriptJSON(
        for recordingFileURL: URL,
        transcriptText: String,
        mode: String,
        language: String,
        endpoint: String,
        audioSeconds: Double,
        postStopWaitSeconds: Double,
        transcriptionSeconds: Double?,
        success: Bool,
        errorMessage: String?
    ) throws -> URL {
        let jsonURL = recordingFileURL.deletingPathExtension().appendingPathExtension("json")
        let payload: [String: Any] = [
            "created_at": ISO8601DateFormatter().string(from: Date()),
            "recording_file": recordingFileURL.lastPathComponent,
            "mode": mode,
            "language": language,
            "endpoint": endpoint,
            "audio_seconds": audioSeconds,
            "post_stop_wait_seconds": postStopWaitSeconds,
            "transcription_seconds": transcriptionSeconds as Any,
            "success": success,
            "error": errorMessage as Any,
            "text": transcriptText
        ]
        let data = try JSONSerialization.data(withJSONObject: payload, options: [.prettyPrinted, .withoutEscapingSlashes])
        try data.write(to: jsonURL, options: .atomic)
        return jsonURL
    }

    private func makeRecordingsDirectory() throws -> URL {
        guard let appSupport = fileManager.urls(for: .applicationSupportDirectory, in: .userDomainMask).first else {
            throw RecordingArchiveError.appSupportDirectoryUnavailable
        }

        let dir = appSupport
            .appendingPathComponent("Dictator", isDirectory: true)
            .appendingPathComponent("Recordings", isDirectory: true)
        try fileManager.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }

    private func pruneRecordings(in directory: URL, policy: RecordingRetentionPolicy) throws {
        let fileURLs = try fileManager.contentsOfDirectory(
            at: directory,
            includingPropertiesForKeys: [.contentModificationDateKey],
            options: [.skipsHiddenFiles]
        )

        let wavs = fileURLs.filter { $0.pathExtension.lowercased() == "wav" }
        guard !wavs.isEmpty else {
            return
        }

        let sorted = try wavs.sorted { lhs, rhs in
            let lDate = try lhs.resourceValues(forKeys: [.contentModificationDateKey]).contentModificationDate ?? .distantPast
            let rDate = try rhs.resourceValues(forKeys: [.contentModificationDateKey]).contentModificationDate ?? .distantPast
            return lDate > rDate
        }

        let toDelete: [URL]
        switch policy {
        case .keepAll:
            toDelete = []
        case .keepLast5:
            toDelete = Array(sorted.dropFirst(5))
        case .keepLast3Days:
            let threshold = Date().addingTimeInterval(-3 * 24 * 60 * 60)
            toDelete = try sorted.filter { url in
                let date = try url.resourceValues(forKeys: [.contentModificationDateKey]).contentModificationDate ?? .distantPast
                return date < threshold
            }
        case .keepLast5Days:
            let threshold = Date().addingTimeInterval(-5 * 24 * 60 * 60)
            toDelete = try sorted.filter { url in
                let date = try url.resourceValues(forKeys: [.contentModificationDateKey]).contentModificationDate ?? .distantPast
                return date < threshold
            }
        }

        for old in toDelete {
            try? fileManager.removeItem(at: old)
            let siblingJSON = old.deletingPathExtension().appendingPathExtension("json")
            try? fileManager.removeItem(at: siblingJSON)
        }
    }

    private func timestampString() -> String {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = TimeZone.current
        formatter.dateFormat = "yyyyMMdd-HHmmss"
        return formatter.string(from: Date())
    }
}

extension Data {
    mutating func appendUInt16LE(_ value: UInt16) {
        var littleEndian = value.littleEndian
        Swift.withUnsafeBytes(of: &littleEndian) { append(contentsOf: $0) }
    }

    mutating func appendUInt32LE(_ value: UInt32) {
        var littleEndian = value.littleEndian
        Swift.withUnsafeBytes(of: &littleEndian) { append(contentsOf: $0) }
    }

    mutating func appendFloat32LE(_ value: Float) {
        var bitPattern = value.bitPattern.littleEndian
        Swift.withUnsafeBytes(of: &bitPattern) { append(contentsOf: $0) }
    }
}
