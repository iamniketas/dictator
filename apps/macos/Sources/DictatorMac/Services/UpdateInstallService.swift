import Foundation

@MainActor
protocol UpdateInstallService {
    var isSupported: Bool { get }
    var engineName: String { get }
    @MainActor func probeForUpdate(currentVersion: String) async throws -> AppUpdateInfo?
    @MainActor func installUpdateNow() -> Bool
}

#if canImport(Sparkle)
import Sparkle

enum SparkleUpdateError: LocalizedError {
    case feedURLMissing
    case probeAlreadyInProgress
    case probeFailed(message: String)

    var errorDescription: String? {
        switch self {
        case .feedURLMissing:
            return "Sparkle feed URL is not configured."
        case .probeAlreadyInProgress:
            return "Update check is already in progress."
        case let .probeFailed(message):
            return message
        }
    }
}

@MainActor
final class SparkleUpdateInstallService: NSObject, UpdateInstallService, SPUUpdaterDelegate {
    private let feedURLString: String?
    private lazy var updaterController = SPUStandardUpdaterController(
        startingUpdater: false,
        updaterDelegate: self,
        userDriverDelegate: nil
    )
    private var probeContinuation: CheckedContinuation<AppUpdateInfo?, Error>?
    private var probeFoundItem: SUAppcastItem?
    private var probeCurrentVersion = ""

    override init() {
        let envFeed = ProcessInfo.processInfo.environment["DICTATOR_SPARKLE_FEED_URL"]?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let plistFeed = Bundle.main.object(forInfoDictionaryKey: "SUFeedURL") as? String
        let configuredFeed = (envFeed?.isEmpty == false ? envFeed : nil) ?? plistFeed
        let cleanFeed: String?
        if let configuredFeed {
            let trimmed = configuredFeed.trimmingCharacters(in: .whitespacesAndNewlines)
            cleanFeed = trimmed.isEmpty ? nil : trimmed
        } else {
            cleanFeed = nil
        }
        feedURLString = cleanFeed
        super.init()
        if cleanFeed != nil {
            updaterController.startUpdater()
        }
    }

    var isSupported: Bool { feedURLString != nil }
    var engineName: String { isSupported ? "Sparkle" : "Release page" }

    func feedURLString(for updater: SPUUpdater) -> String? {
        feedURLString
    }

    func probeForUpdate(currentVersion: String) async throws -> AppUpdateInfo? {
        guard isSupported else {
            throw SparkleUpdateError.feedURLMissing
        }
        guard probeContinuation == nil else {
            throw SparkleUpdateError.probeAlreadyInProgress
        }

        probeFoundItem = nil
        probeCurrentVersion = currentVersion

        return try await withCheckedThrowingContinuation { continuation in
            probeContinuation = continuation
            updaterController.updater.checkForUpdateInformation()
        }
    }

    func installUpdateNow() -> Bool {
        guard isSupported else {
            return false
        }
        _ = updaterController
        updaterController.checkForUpdates(nil)
        return true
    }

    func updater(_ updater: SPUUpdater, didFindValidUpdate item: SUAppcastItem) {
        probeFoundItem = item
    }

    func updaterDidNotFindUpdate(_ updater: SPUUpdater) {
        probeFoundItem = nil
    }

    func updater(_ updater: SPUUpdater, didFinishUpdateCycleFor updateCheck: SPUUpdateCheck, error: Error?) {
        guard updateCheck == .updateInformation else {
            return
        }
        guard let continuation = probeContinuation else {
            return
        }
        probeContinuation = nil

        if let error, probeFoundItem == nil {
            continuation.resume(throwing: SparkleUpdateError.probeFailed(message: error.localizedDescription))
            return
        }

        if let item = probeFoundItem {
            let version = normalize(version: item.displayVersionString)
            let current = normalize(version: probeCurrentVersion)
            if isVersion(version, greaterThan: current) {
                let url = item.infoURL ?? item.releaseNotesURL ?? item.fileURL ?? URL(string: "https://github.com/iamniketas/dictator/releases/latest")
                if let url {
                    continuation.resume(
                        returning: AppUpdateInfo(
                            version: version,
                            htmlURL: url,
                            notes: item.itemDescription ?? item.title ?? ""
                        )
                    )
                } else {
                    continuation.resume(returning: nil)
                }
            } else {
                continuation.resume(returning: nil)
            }
        } else {
            continuation.resume(returning: nil)
        }
    }

    private func normalize(version: String) -> String {
        version.trimmingCharacters(in: .whitespacesAndNewlines)
            .replacingOccurrences(of: "v", with: "", options: [.anchored, .caseInsensitive])
    }

    private func isVersion(_ lhs: String, greaterThan rhs: String) -> Bool {
        let left = lhs.split(separator: ".").map { Int($0) ?? 0 }
        let right = rhs.split(separator: ".").map { Int($0) ?? 0 }
        let count = max(left.count, right.count)
        for index in 0..<count {
            let l = index < left.count ? left[index] : 0
            let r = index < right.count ? right[index] : 0
            if l != r {
                return l > r
            }
        }
        return false
    }
}

#else

final class SparkleUpdateInstallService: UpdateInstallService {
    var isSupported: Bool { false }
    var engineName: String { "Release page" }

    @MainActor
    func probeForUpdate(currentVersion: String) async throws -> AppUpdateInfo? {
        nil
    }

    @MainActor
    func installUpdateNow() -> Bool {
        false
    }
}

#endif
