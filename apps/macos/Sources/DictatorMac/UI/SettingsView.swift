import SwiftUI

struct SettingsView: View {
    @ObservedObject var model: AppModel
    @ObservedObject var whisperKit: WhisperKitTranscriptionService

    var body: some View {
        TabView {
            TranscriptionTab(model: model, whisperKit: whisperKit)
                .tabItem { Label("Transcription", systemImage: "waveform") }

            RecordingTab(model: model)
                .tabItem { Label("Recording", systemImage: "mic") }

            LLMTab(model: model)
                .tabItem { Label("LLM", systemImage: "sparkles") }

            AboutTab()
                .tabItem { Label("About", systemImage: "info.circle") }
        }
        .padding(20)
        .frame(width: 480)
    }
}

// MARK: - Transcription tab

private struct TranscriptionTab: View {
    @ObservedObject var model: AppModel
    @ObservedObject var whisperKit: WhisperKitTranscriptionService

    var body: some View {
        Form {
            Section {
                Picker("Backend", selection: $model.transcriptionBackend) {
                    ForEach(TranscriptionBackend.allCases, id: \.self) { backend in
                        Text(backend.displayName).tag(backend)
                    }
                }
                .pickerStyle(.radioGroup)
            } header: {
                Text("Transcription Backend")
            }

            switch model.transcriptionBackend {
            case .whisperKit: whisperKitSection
            case .mlx:        mlxSection
            case .http:       httpSection
            }

            Section {
                TextField("Language code (e.g. ru, en, de)", text: $model.transcriptionLanguage)
                    .textFieldStyle(.roundedBorder)
                Text("Use ISO 639-1 two-letter codes. Empty = auto-detect.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            } header: {
                Text("Language")
            }
        }
        .formStyle(.grouped)
    }

    // MARK: - WhisperKit section

    private var whisperKitSection: some View {
        Section {
            Picker("Model", selection: $model.whisperKitModelName) {
                ForEach(WhisperKitModelInfo.available) { info in
                    Text("\(info.displayName)  (~\(info.approxSizeMB) MB)")
                        .tag(info.id)
                }
            }

            HStack {
                Circle()
                    .fill(loadStateColor)
                    .frame(width: 8, height: 8)
                Text(whisperKit.loadState.statusText)
                    .foregroundStyle(.secondary)
                Spacer()
                if whisperKit.loadState == .downloading || whisperKit.loadState == .loading {
                    ProgressView()
                        .controlSize(.small)
                } else {
                    Button("Load Model") {
                        Task {
                            await whisperKit.loadModel(model.whisperKitModelName)
                        }
                    }
                    .disabled(isLoadDisabled)
                }
            }

            if case .ready = whisperKit.loadState {
                Button("Unload from Memory", role: .destructive) {
                    whisperKit.unload()
                }
            }
        } header: {
            Text("WhisperKit (on-device)")
        } footer: {
            Text("Models download once from HuggingFace to ~/Library/Application Support/huggingface/models/. large-v3-turbo is recommended for Russian.")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }

    private var loadStateColor: Color {
        switch whisperKit.loadState {
        case .ready:               return .green
        case .downloading, .loading: return .orange
        case .failed:              return .red
        case .idle:                return .secondary
        }
    }

    private var isLoadDisabled: Bool {
        whisperKit.loadState == .loading || whisperKit.loadState == .downloading
    }

    // MARK: - MLX section

    private var mlxSection: some View {
        Section {
            // Server status indicator
            HStack {
                Circle()
                    .fill(mlxStatusColor)
                    .frame(width: 8, height: 8)
                Text(mlxStatusText)
                    .foregroundStyle(.secondary)
                Spacer()
                Button("Check") { model.probeMlxServer() }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
            }
            .onAppear { model.probeMlxServer() }

            TextField("Endpoint URL", text: $model.mlxEndpointURL)
                .textFieldStyle(.roundedBorder)

            // Model picker: preset recommended models + current custom value
            Picker("Model", selection: $model.mlxModelID) {
                ForEach(MLXModelInfo.recommended) { info in
                    VStack(alignment: .leading, spacing: 1) {
                        Text(info.displayName)
                        Text(info.note)
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                    }
                    .tag(info.id)
                }
                if !MLXModelInfo.recommended.map(\.id).contains(model.mlxModelID) {
                    Text(model.mlxModelID).tag(model.mlxModelID)
                }
            }

            // Allow typing a custom HuggingFace model ID
            TextField("Custom model ID (HuggingFace)", text: $model.mlxModelID)
                .textFieldStyle(.roundedBorder)
                .font(.system(size: 12, design: .monospaced))

        } header: {
            Text("MLX Server (Contora / OpenAI API)")
        } footer: {
            Text("Requires Contora with MLX backend running. Config auto-loaded from ~/Library/Application Support/NiketasAI/runtime/transcription-server.json when available.")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }

    private var mlxStatusColor: Color {
        switch model.mlxServerReachable {
        case .none:  return .secondary
        case true:   return .green
        case false:  return .red
        }
    }

    private var mlxStatusText: String {
        switch model.mlxServerReachable {
        case .none:  return "Not checked"
        case true:   return "Server reachable ✓"
        case false:  return "Server not running"
        }
    }

    // MARK: - HTTP section

    private var httpSection: some View {
        Section {
            TextField("Endpoint URL", text: $model.httpEndpointURL)
                .textFieldStyle(.roundedBorder)
            Text("Legacy Python whisper-server. See shared/whisper-server/.")
                .font(.caption)
                .foregroundStyle(.secondary)
        } header: {
            Text("HTTP Server (legacy)")
        }
    }
}

// MARK: - Recording tab

private struct RecordingTab: View {
    @ObservedObject var model: AppModel

    var body: some View {
        Form {
            Section {
                Toggle("Enable streaming mode", isOn: $model.streamingEnabled)
                if model.streamingEnabled {
                    Picker("Chunk size", selection: $model.chunkSeconds) {
                        Text("3 s").tag(3)
                        Text("8 s").tag(8)
                        Text("15 s").tag(15)
                    }
                    .pickerStyle(.segmented)
                }
            } header: {
                Text("Streaming")
            } footer: {
                Text("Streaming mode transcribes audio in chunks during recording for faster perceived latency.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            Section {
                Text("Hotkey: Cmd+Shift+D or Ctrl+Shift+D")
                    .foregroundStyle(.secondary)
                Picker("Mode", selection: $model.hotkeyMode) {
                    Text("Smart (tap = toggle, hold = PTT)").tag(HotkeyManager.TriggerMode.smart)
                    Text("Toggle").tag(HotkeyManager.TriggerMode.toggle)
                    Text("Push-to-Talk").tag(HotkeyManager.TriggerMode.ptt)
                }
                .pickerStyle(.radioGroup)
                Text("Smart: tap < 300 ms = start/stop. Hold ≥ 300 ms = push-to-talk (stops on release).")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            } header: {
                Text("Hotkey")
            }

            Section {
                PermissionsView(permissions: model.permissions)
            } header: {
                Text("Permissions")
            }
        }
        .formStyle(.grouped)
    }
}

// MARK: - Permissions sub-view

private struct PermissionsView: View {
    @ObservedObject var permissions: PermissionState

    var body: some View {
        HStack {
            permissionRow("Microphone", status: permissions.microphone)
            Spacer()
            if permissions.microphone != .granted {
                Button("Request") { permissions.requestMicrophone() }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
            }
        }
        HStack {
            permissionRow("Accessibility", status: permissions.accessibility)
            Spacer()
            if permissions.accessibility != .granted {
                Button("Open System Settings") { permissions.requestAccessibilityPrompt() }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
            }
        }
    }

    private func permissionRow(_ name: String, status: PermissionStatus) -> some View {
        HStack(spacing: 6) {
            Circle()
                .fill(status == .granted ? Color.green : status == .denied ? Color.red : Color.orange)
                .frame(width: 8, height: 8)
            Text("\(name): \(status.rawValue)")
        }
    }
}

// MARK: - LLM tab

private struct LLMTab: View {
    @ObservedObject var model: AppModel

    var body: some View {
        Form {
            Section {
                Toggle("Enable LLM grammar correction", isOn: $model.llmEnabled)
            } header: {
                Text("Ollama Post-Processing")
            } footer: {
                Text("When enabled, the raw transcript is sent to a local Ollama model for grammar and punctuation correction before being pasted. Requires Ollama running locally.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            if model.llmEnabled {
                Section {
                    TextField("Endpoint URL", text: $model.llmEndpointURL)
                        .textFieldStyle(.roundedBorder)
                    Text("Default: http://127.0.0.1:11434")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                } header: {
                    Text("Ollama Server")
                }

                Section {
                    TextField("Model name", text: $model.llmModel)
                        .textFieldStyle(.roundedBorder)
                    Text("e.g. llama3, mistral, gemma3. Must be pulled in Ollama first.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                } header: {
                    Text("Model")
                }

                Section {
                    TextEditor(text: $model.llmSystemPrompt)
                        .font(.system(size: 12, design: .monospaced))
                        .frame(minHeight: 80)
                        .overlay(RoundedRectangle(cornerRadius: 4).stroke(.separator, lineWidth: 0.5))
                    Button("Reset to default") {
                        model.llmSystemPrompt = LLMService.defaultCorrectionPrompt
                    }
                    .buttonStyle(.borderless)
                    .font(.caption)
                } header: {
                    Text("System Prompt")
                } footer: {
                    Text("The raw transcript is appended after this prompt.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
        }
        .formStyle(.grouped)
    }
}

// MARK: - About tab

private struct AboutTab: View {
    var body: some View {
        Form {
            Section {
                LabeledContent("Version", value: "macOS prototype")
                LabeledContent("Source", value: "github.com/dictator")
            }
            Section {
                Button("Open Recordings Folder") {
                    if let url = try? RecordingArchiveService().recordingsDirectory() {
                        NSWorkspace.shared.open(url)
                    }
                }
            }
        }
        .formStyle(.grouped)
    }
}
