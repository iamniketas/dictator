import Foundation

struct AppUpdateInfo: Equatable {
    let version: String
    let htmlURL: URL
    let notes: String
}

protocol UpdateService {
    func checkForUpdate(currentVersion: String) async throws -> AppUpdateInfo?
}

enum UpdateServiceError: LocalizedError {
    case invalidResponse

    var errorDescription: String? {
        switch self {
        case .invalidResponse:
            return "Invalid update server response."
        }
    }
}

final class GitHubReleaseUpdateService: UpdateService {
    private let repoOwner: String
    private let repoName: String

    init(repoOwner: String = "iamniketas", repoName: String = "dictator") {
        self.repoOwner = repoOwner
        self.repoName = repoName
    }

    func checkForUpdate(currentVersion: String) async throws -> AppUpdateInfo? {
        guard let url = URL(string: "https://api.github.com/repos/\(repoOwner)/\(repoName)/releases/latest") else {
            throw UpdateServiceError.invalidResponse
        }

        var request = URLRequest(url: url)
        request.setValue("application/vnd.github+json", forHTTPHeaderField: "Accept")
        request.timeoutInterval = 20

        let (data, response) = try await URLSession.shared.data(for: request)
        guard let http = response as? HTTPURLResponse, (200...299).contains(http.statusCode) else {
            throw UpdateServiceError.invalidResponse
        }

        let release = try JSONDecoder().decode(GitHubRelease.self, from: data)
        guard !release.draft, !release.prerelease else {
            return nil
        }

        let latest = normalize(version: release.tagName)
        let current = normalize(version: currentVersion)
        guard isVersion(latest, greaterThan: current),
              let pageURL = URL(string: release.htmlURL) else {
            return nil
        }

        return AppUpdateInfo(
            version: latest,
            htmlURL: pageURL,
            notes: release.body ?? ""
        )
    }

    private func normalize(version: String) -> String {
        version.trimmingCharacters(in: .whitespacesAndNewlines)
            .replacingOccurrences(of: "v", with: "", options: [.anchored, .caseInsensitive])
    }

    private func isVersion(_ lhs: String, greaterThan rhs: String) -> Bool {
        let left = lhs.split(separator: ".").map { Int($0) ?? 0 }
        let right = rhs.split(separator: ".").map { Int($0) ?? 0 }
        let count = max(left.count, right.count)
        for i in 0..<count {
            let l = i < left.count ? left[i] : 0
            let r = i < right.count ? right[i] : 0
            if l != r { return l > r }
        }
        return false
    }
}

private struct GitHubRelease: Decodable {
    let tagName: String
    let htmlURL: String
    let body: String?
    let draft: Bool
    let prerelease: Bool

    enum CodingKeys: String, CodingKey {
        case tagName = "tag_name"
        case htmlURL = "html_url"
        case body
        case draft
        case prerelease
    }
}
