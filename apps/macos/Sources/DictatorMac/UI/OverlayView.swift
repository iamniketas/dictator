import SwiftUI

struct DashboardView: View {
    @ObservedObject var model: AppModel

    var body: some View {
        content
            .padding(.horizontal, 8)
            .padding(.vertical, 6)
            .frame(width: 280, height: 40)
            .background(
                RoundedRectangle(cornerRadius: 12, style: .continuous)
                    .fill(.ultraThinMaterial)
                    .overlay(
                        RoundedRectangle(cornerRadius: 12, style: .continuous)
                            .strokeBorder(
                                LinearGradient(
                                    colors: [stateAccent.opacity(0.28), .white.opacity(0.18)],
                                    startPoint: .topLeading,
                                    endPoint: .bottomTrailing
                                ),
                                lineWidth: 1
                            )
                    )
            )
            .shadow(color: .black.opacity(0.12), radius: 10, x: 0, y: 4)
            .animation(.easeInOut(duration: 0.18), value: model.isRecording)
            .animation(.easeInOut(duration: 0.18), value: model.isTranscribing)
            .animation(.easeInOut(duration: 0.18), value: model.isStreamingChunkTranscribing)
    }

    @ViewBuilder
    private var content: some View {
        if model.isRecording {
            WaveformView(bars: model.waveformBars)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
        } else if model.isFinalizingStop || model.isTranscribing || model.isStreamingChunkTranscribing {
            HStack(spacing: 8) {
                ProgressView()
                    .scaleEffect(0.78)

                VStack(alignment: .leading, spacing: 2) {
                    Text("Processing")
                        .font(.system(size: 13, weight: .semibold, design: .rounded))
                    if model.estimatedProcessingTotalSeconds >= 10, model.estimatedProcessingRemainingSeconds > 0 {
                        Text("About \(formatSeconds(model.estimatedProcessingRemainingSeconds)) remaining")
                            .font(.system(size: 12, weight: .regular, design: .rounded))
                            .foregroundStyle(.secondary)
                    } else {
                        Text("Please wait...")
                            .font(.system(size: 12, weight: .regular, design: .rounded))
                            .foregroundStyle(.secondary)
                    }
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        } else {
            HStack(spacing: 8) {
                Image(systemName: "checkmark.circle.fill")
                    .foregroundStyle(Color.green)
                Text(model.statusMessage)
                    .font(.system(size: 13, weight: .medium, design: .rounded))
                    .lineLimit(1)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private var stateAccent: Color {
        if model.isRecording { return .red }
        if model.isFinalizingStop || model.isTranscribing || model.isStreamingChunkTranscribing { return .orange }
        return .blue
    }

    private func formatSeconds(_ value: Double) -> String {
        let rounded = Int(max(0, value))
        return "\(rounded)s"
    }
}

private struct WaveformView: View {
    let bars: [Double]

    var body: some View {
        GeometryReader { geometry in
            let spacing: CGFloat = 1
            let count = max(1, bars.count)
            let availableWidth = max(0, geometry.size.width - (CGFloat(count - 1) * spacing))
            let barWidth = max(1.5, availableWidth / CGFloat(count))
            HStack(spacing: spacing) {
                ForEach(Array(bars.enumerated()), id: \.offset) { _, value in
                    Capsule(style: .continuous)
                        .fill(Color.red.opacity(0.86))
                        .frame(width: barWidth, height: barHeight(value))
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
    }

    private func barHeight(_ normalized: Double) -> CGFloat {
        let clamped = min(max(normalized, 0), 1)
        return CGFloat(4 + (clamped * 30))
    }
}
