import AppKit
import SwiftUI

struct SettingsView: View {
    @ObservedObject var model: AppModel

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                GroupBox("General") {
                    VStack(alignment: .leading, spacing: 10) {
                        Toggle("Open at Login", isOn: $model.launchAtLogin)
                        Toggle("Enable Live Transcription by Default", isOn: $model.streamingEnabled)
                    }
                    .padding(.top, 4)
                }

                GroupBox("Transcription") {
                    VStack(alignment: .leading, spacing: 10) {
                        Picker("Engine", selection: $model.transcriptionBackendPreference) {
                            ForEach(TranscriptionBackendPreference.allCases) { backend in
                                Text(backend.title).tag(backend)
                            }
                        }
                        Picker("Chunk Duration", selection: $model.chunkSeconds) {
                            Text("3s").tag(3)
                            Text("8s").tag(8)
                            Text("15s").tag(15)
                        }
                        .pickerStyle(.segmented)
                        TextField("HTTP fallback endpoint", text: $model.transcriptionEndpoint)
                            .textFieldStyle(.roundedBorder)
                        TextField("Language code (e.g. ru, en)", text: $model.transcriptionLanguage)
                            .textFieldStyle(.roundedBorder)
                        LabeledContent("Active engine") {
                            Text(model.activeTranscriptionBackend)
                                .foregroundStyle(.secondary)
                        }
                    }
                    .padding(.top, 4)
                }

                GroupBox("Text Injection") {
                    VStack(alignment: .leading, spacing: 10) {
                        Picker("Mode", selection: $model.textInjectionMode) {
                            ForEach(TextInjectionMode.allCases) { mode in
                                Text(mode.title).tag(mode)
                            }
                        }
                        LabeledContent("Last action") {
                            Text(model.lastInjectionStatus)
                                .foregroundStyle(.secondary)
                                .lineLimit(2)
                                .multilineTextAlignment(.trailing)
                        }
                    }
                    .padding(.top, 4)
                }

                GroupBox("Permissions Onboarding") {
                    VStack(alignment: .leading, spacing: 10) {
                        LabeledContent("Microphone") {
                            Text(model.permissions.microphone.rawValue)
                                .foregroundStyle(model.permissions.microphone == .granted ? .green : .secondary)
                        }
                        LabeledContent("Accessibility") {
                            Text(model.permissions.accessibility.rawValue)
                                .foregroundStyle(model.permissions.accessibility == .granted ? .green : .secondary)
                        }

                        HStack {
                            Button("Request Microphone") {
                                model.permissions.requestMicrophone()
                            }
                            Button("Request Accessibility") {
                                model.permissions.requestAccessibilityPrompt()
                            }
                            Button("Refresh Status") {
                                model.permissions.refresh()
                            }
                        }

                        HStack {
                            Button("Open Accessibility Settings…") {
                                openAccessibilitySettings()
                            }
                            Button("Open Microphone Settings…") {
                                openMicrophoneSettings()
                            }
                        }
                    }
                    .padding(.top, 4)
                }

                Text("Dictator stays minimal by default. The floating overlay appears only while recording or processing.")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
            }
            .padding(20)
        }
        .frame(width: 560, height: 620)
        .onAppear {
            model.permissions.refresh()
        }
    }

    private func openAccessibilitySettings() {
        guard let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility") else {
            return
        }
        NSWorkspace.shared.open(url)
    }

    private func openMicrophoneSettings() {
        guard let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone") else {
            return
        }
        NSWorkspace.shared.open(url)
    }
}
