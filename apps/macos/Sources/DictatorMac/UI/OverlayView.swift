import SwiftUI

struct DashboardView: View {
    @ObservedObject var model: AppModel

    var body: some View {
        content
            .padding(.horizontal, 8)
            .padding(.vertical, 6)
            .frame(width: 170, height: 40)
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
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .center)
        } else if model.isFinalizingStop || model.isTranscribing || model.isStreamingChunkTranscribing {
            HStack(spacing: 8) {
                ProgressView()
                    .scaleEffect(0.78)
                Text("Processing...")
                    .font(.system(size: 13, weight: .semibold, design: .rounded))
            }
            .frame(maxWidth: .infinity, alignment: .center)
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
                        .frame(width: barWidth, height: barHeight(value, availableHeight: geometry.size.height))
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .center)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .center)
    }

    private func barHeight(_ normalized: Double, availableHeight: CGFloat) -> CGFloat {
        let clamped = min(max(normalized, 0), 1)
        let verticalPadding: CGFloat = 2
        let minHeight: CGFloat = 2
        let maxHeight = max(minHeight, availableHeight - (verticalPadding * 2))
        return minHeight + (maxHeight - minHeight) * clamped
    }
}
