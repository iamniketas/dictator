import SwiftUI

/// Floating ticker shown during recording and transcription.
/// Borderless, 320×36 px, floats above all windows on all spaces.
struct DashboardView: View {
    @ObservedObject var model: AppModel

    var body: some View {
        HStack(spacing: 8) {
            if model.isRecording {
                WaveformBarsView(amplitude: model.micAmplitude)
                    .frame(width: 28, height: 20)
            } else {
                Circle()
                    .fill(indicatorColor)
                    .frame(width: 8, height: 8)
            }

            Text(tickerText)
                .font(.system(size: 14, weight: .medium, design: .rounded))
                .lineLimit(1)
                .truncationMode(.head)
                .frame(maxWidth: .infinity, alignment: .trailing)
                .textSelection(.enabled)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .frame(width: 320, height: 36)
        .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
    }

    private var indicatorColor: Color {
        if model.isRecording { return .red }
        if model.isFinalizingStop || model.isTranscribing || model.isStreamingChunkTranscribing {
            return .orange
        }
        return Color.secondary.opacity(0.35)
    }

    private var tickerText: String {
        if model.isRecording {
            // After 30s show a one-time hint about hands-free mode.
            if model.showLongRecordingHint {
                return "Tap again for hands-free mode ✦"
            }
            if !model.lastTranscript.isEmpty, model.lastTranscript != "Recording in progress..." {
                return model.lastTranscript.replacingOccurrences(of: "\n", with: " ")
            }
            return "Recording... \(String(format: "%.0f", model.recordingSeconds))s"
        }
        if model.isFinalizingStop || model.isTranscribing || model.isStreamingChunkTranscribing {
            return "Transcribing... \(String(format: "%.1f", model.postStopWaitSeconds))s"
        }
        if model.isCorrectingLLM {
            return "Correcting with LLM…"
        }
        if model.lastTranscript.isEmpty || model.lastTranscript == "No transcript yet." {
            return model.statusMessage
        }
        return model.lastTranscript.replacingOccurrences(of: "\n", with: " ")
    }
}

// MARK: - Waveform bars

/// Five animated vertical bars whose heights track the microphone RMS amplitude.
private struct WaveformBarsView: View {
    let amplitude: Float

    // Per-bar height scale factors — outer bars shorter for organic look.
    private let scales: [CGFloat] = [0.55, 0.80, 1.0, 0.80, 0.55]
    // Per-bar minimum heights to prevent bars from fully collapsing in silence.
    private let minimums: [CGFloat] = [3, 4, 4, 4, 3]
    private let maxBarHeight: CGFloat = 18

    var body: some View {
        HStack(spacing: 3) {
            ForEach(0..<5, id: \.self) { i in
                Capsule()
                    .fill(Color.red.opacity(0.88))
                    .frame(width: 3, height: barHeight(for: i))
                    .animation(.easeOut(duration: 0.09), value: amplitude)
            }
        }
    }

    private func barHeight(for index: Int) -> CGFloat {
        // Boost typical speech RMS (0.01–0.3) to fill the bar range nicely.
        let boosted = min(CGFloat(amplitude) * 5.0, 1.0)
        let height = boosted * maxBarHeight * scales[index]
        return max(minimums[index], height)
    }
}
