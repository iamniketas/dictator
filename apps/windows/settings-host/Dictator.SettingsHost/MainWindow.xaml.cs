using System.ComponentModel;
using System.Collections.ObjectModel;
using System.Diagnostics;
using System.Globalization;
using System.Linq;
using System.Management;
using System.Net.Http;
using System.Net.Http.Headers;
using System.Net;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Text.Json;
using System.Text.Json.Serialization;
using Microsoft.UI;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;

namespace Dictator.SettingsHost;

public sealed partial class MainWindow : Window
{
    [DllImport("shell32.dll", CharSet = CharSet.Unicode)]
    private static extern int SetCurrentProcessExplicitAppUserModelID(string appID);

    private readonly string _configPath;
    private string _modelsDir;
    private readonly string _storePath;
    private string _audioHistoryDir;
    private string _transcriptsDir;
    private readonly bool _startInOnboarding;

    private readonly ObservableCollection<CatalogModelItem> _catalogModels = new();
    private readonly ObservableCollection<HistoryEntryItem> _historyEntries = new();
    private readonly ObservableCollection<CorrectionEntryItem> _correctionEntries = new();
    private readonly List<CatalogModelItem> _catalogSource = CatalogModelItem.DefaultCatalog();
    private readonly Dictionary<string, CancellationTokenSource> _downloadJobs = new(StringComparer.OrdinalIgnoreCase);
    private readonly HashSet<string> _sizeProbeQueued = new(StringComparer.OrdinalIgnoreCase);
    private int _lastHardwareScore = 5;
    private string RuntimeProfilesDir => Path.Combine(_modelsDir, "runtimes", "profiles");

    private bool _uiReady;
    private bool _closeHintShown;
    private bool _allowClose;
    private bool _runtimeDiagnosticsLoaded;
    private bool _historyLoaded;
    private HistoryEntryItem? _selectedHistory;

    public MainWindow()
    {
        InitializeComponent();

        SetCurrentProcessExplicitAppUserModelID("Dictator.SettingsHost");

        (_configPath, _modelsDir, _storePath, var historyRoot, var audioDirArg, var transcriptsDirArg, _startInOnboarding) = ParseArgs();
        _audioHistoryDir = string.IsNullOrWhiteSpace(audioDirArg)
            ? Path.Combine(historyRoot, "audio")
            : audioDirArg;
        _transcriptsDir = string.IsNullOrWhiteSpace(transcriptsDirArg)
            ? Path.Combine(historyRoot, "transcripts")
            : transcriptsDirArg;
        LoadStorageConfig();

        Title = "Dictator Settings";
        SetWindowSize(1120, 800);
        TrySetWindowIcon();
        AttachCloseHint();

        SelectNavByTag(_startInOnboarding ? "dashboard" : "models");
        CatalogItems.ItemsSource = _catalogModels;
        HistoryList.ItemsSource = _historyEntries;
        CorrectionsList.ItemsSource = _correctionEntries;

        RefreshAll();
        _uiReady = true;
    }

    private static (string configPath, string modelsDir, string storePath, string historyDir, string audioDir, string transcriptsDir, bool onboarding) ParseArgs()
    {
        var args = Environment.GetCommandLineArgs();
        string GetValue(string key, string fallback)
        {
            for (int i = 0; i < args.Length - 1; i++)
            {
                if (string.Equals(args[i], key, StringComparison.OrdinalIgnoreCase)) return args[i + 1];
            }
            return fallback;
        }

        var local = Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData);
        var docs = Environment.GetFolderPath(Environment.SpecialFolder.MyDocuments);
        var onboarding = args.Any(a => string.Equals(a, "--onboarding", StringComparison.OrdinalIgnoreCase));
        return (
            GetValue("--config", Path.Combine(local, "dictator", "config.toml")),
            GetValue("--models-dir", Path.Combine(local, "AudioModels")),
            GetValue("--store-path", Path.Combine(local, "AudioModels", "shared_model_store.v1.json")),
            GetValue("--history-dir", Path.Combine(docs, "Dictator", "History")),
            GetValue("--audio-dir", string.Empty),
            GetValue("--transcripts-dir", string.Empty),
            onboarding
        );
    }

    private void AttachCloseHint()
    {
        try
        {
            var hwnd = WinRT.Interop.WindowNative.GetWindowHandle(this);
            var windowId = Microsoft.UI.Win32Interop.GetWindowIdFromWindow(hwnd);
            var appWindow = AppWindow.GetFromWindowId(windowId);
            appWindow.Closing += OnAppWindowClosing;
        }
        catch
        {
        }
    }

    private async void OnAppWindowClosing(AppWindow sender, AppWindowClosingEventArgs args)
    {
        if (_allowClose || _closeHintShown) return;

        args.Cancel = true;
        _closeHintShown = true;
        try
        {
            var dialog = new ContentDialog
            {
                Title = "Dictator stays in tray",
                Content = "Closing this window does not stop Dictator. To fully exit, right-click the tray icon and choose Exit.",
                CloseButtonText = "Got it",
                XamlRoot = Content.XamlRoot
            };
            await dialog.ShowAsync();
        }
        catch
        {
        }

        _allowClose = true;
        Close();
    }

    private void TrySetWindowIcon()
    {
        try
        {
            var iconPath = Path.Combine(AppContext.BaseDirectory, "Assets", "Dictator.ico");
            if (!File.Exists(iconPath)) return;

            var hwnd = WinRT.Interop.WindowNative.GetWindowHandle(this);
            var windowId = Microsoft.UI.Win32Interop.GetWindowIdFromWindow(hwnd);
            var appWindow = AppWindow.GetFromWindowId(windowId);
            appWindow.SetIcon(iconPath);
        }
        catch { }
    }

    private void SetWindowSize(int width, int height)
    {
        var hWnd = WinRT.Interop.WindowNative.GetWindowHandle(this);
        var windowId = Microsoft.UI.Win32Interop.GetWindowIdFromWindow(hWnd);
        var appWindow = AppWindow.GetFromWindowId(windowId);
        appWindow.Resize(new Windows.Graphics.SizeInt32(width, height));
    }

    private void RefreshAll()
    {
        Directory.CreateDirectory(_modelsDir);
        Directory.CreateDirectory(_audioHistoryDir);
        Directory.CreateDirectory(_transcriptsDir);

        LoadConfigFields();
        RefreshStorageCards();
        RefreshHardwareDiagnostics();
        _runtimeDiagnosticsLoaded = true;
        LoadCatalog();
        LoadCorrectionsConfigFields();
        RefreshCorrectionsList();
        RefreshWelcomeSummary();
        RefreshDictationGuidance();
        SyncSharedModelStore();

        AboutText.Text = "Dictator Settings Host (WinUI 3). Changes are applied immediately.";
    }

    private void RefreshWelcomeSummary()
    {
        var modelPath = (GetTomlString("whisper", "model_path") ?? string.Empty).Trim();
        var hasModel = !string.IsNullOrWhiteSpace(modelPath) && (File.Exists(modelPath) || Directory.Exists(modelPath));
        WelcomeModelStatusText.Text = hasModel
            ? $"Model: ready ({Path.GetFileName(modelPath)})"
            : "Model: not configured yet (open Models and install one)";

        var runtimePref = (GetTomlString("runtime", "preference") ?? "auto").Trim();
        WelcomeRuntimeStatusText.Text = runtimePref switch
        {
            "force_gpu" => "Runtime: Force GPU",
            "force_cpu" => "Runtime: Force CPU",
            _ => "Runtime: Auto (recommended)",
        };

        var hotkey = (GetTomlString("hotkey", "key") ?? "right_ctrl").Trim();
        var hotkeyLabel = hotkey.Replace('_', ' ');
        WelcomeHotkeyStatusText.Text = $"Hotkey: {hotkeyLabel}";
    }

    private void RefreshDictationGuidance()
    {
        var streaming = StreamingToggle.IsOn;
        var chunk = (int)Math.Clamp((int)Math.Round(ChunkSecondsBox.Value), 3, 60);
        var runtime = (RuntimeModeCombo.SelectedItem as ComboBoxItem)?.Tag as string ?? "auto";
        var injection = (InjectionCombo.SelectedItem as ComboBoxItem)?.Tag as string ?? "direct";
        var llm = LlmToggle.IsOn;
        var overlayEnabled = PostOverlayToggle.IsOn;
        var overlaySecs = (int)Math.Clamp((int)Math.Round(PostOverlaySecondsBox.Value), 1, 15);

        var speedTier = _lastHardwareScore >= 8 ? "high-performance" : _lastHardwareScore >= 5 ? "balanced" : "compatibility-first";
        DictationProfileSummaryText.Text =
            $"Current profile: {(streaming ? "Streaming" : "Full transcription")} | Runtime: {runtime} | Hardware: {speedTier}.";

        DictationLatencyHintText.Text = streaming
            ? $"Latency hint: chunk={chunk}s. Smaller chunks react faster but may reduce stability on long phrases."
            : "Latency hint: full mode waits for stop, then runs one stable pass with usually better consistency.";

        DictationQualityHintText.Text = llm
            ? "Quality hint: Ollama cleanup is ON. Text is cleaner, final result appears a bit later."
            : "Quality hint: direct output mode is ON. Fastest result, no extra cleanup.";

        ChunkHintText.Text = chunk switch
        {
            <= 4 => "Chunk guidance: aggressive low-latency mode. Best for short commands.",
            <= 10 => "Chunk guidance: balanced mode. Good for continuous dictation.",
            _ => "Chunk guidance: quality-biased streaming. Slower updates but more context per chunk."
        };

        InjectionHintText.Text = injection switch
        {
            "clipboard" => "Injection hint: safer compatibility via clipboard paste.",
            "clipboard_enter" => "Injection hint: clipboard paste + Enter, convenient for chat workflows.",
            _ => "Injection hint: direct keystroke injection for fastest insertion."
        };

        OverlayHintText.Text = overlayEnabled
            ? $"Overlay hint: confirmation panel is visible for {overlaySecs}s after insertion."
            : "Overlay hint: post-transcription confirmation is disabled for minimal interruption.";
    }

    private void SelectNavByTag(string tag)
    {
        foreach (var item in Nav.MenuItems.OfType<NavigationViewItem>())
        {
            if (string.Equals(item.Tag as string, tag, StringComparison.OrdinalIgnoreCase))
            {
                Nav.SelectedItem = item;
                break;
            }
        }
    }

    private void LoadStorageConfig()
    {
        _audioHistoryDir = GetTomlString("storage", "audio_history_dir")?.Trim() ?? _audioHistoryDir;
        _transcriptsDir = GetTomlString("storage", "transcripts_dir")?.Trim() ?? _transcriptsDir;
    }

    private void LoadCatalog()
    {
        var activePath = NormalizePath(GetTomlString("whisper", "model_path") ?? string.Empty);
        var backend = (GetTomlString("whisper", "backend") ?? string.Empty).Trim();
        var runtimePref = (GetTomlString("runtime", "preference") ?? "auto").Trim();
        var hasCudaHint = (HardwareGpuText.Text ?? string.Empty).Contains("NVIDIA", StringComparison.OrdinalIgnoreCase);
        var store = LoadStoreSnapshot();
        var storeById = store?.InstalledModels
            .GroupBy(m => m.Id, StringComparer.OrdinalIgnoreCase)
            .ToDictionary(g => g.Key, g => g.First(), StringComparer.OrdinalIgnoreCase)
            ?? new Dictionary<string, StoreModelDoc>(StringComparer.OrdinalIgnoreCase);
        var existing = _catalogModels.ToDictionary(m => m.Id, StringComparer.OrdinalIgnoreCase);
        var ordered = _catalogSource
            .Select(m => new
            {
                Model = m,
                Fit = EstimateSuitability(m, _lastHardwareScore, hasCudaHint, runtimePref),
            })
            .OrderByDescending(x => x.Fit)
            .ThenByDescending(x => x.Model.Accuracy10)
            .ThenByDescending(x => x.Model.Speed10)
            .Select(x => x.Model);

        var next = new List<CatalogModelItem>();

        foreach (var source in ordered)
        {
            var localPath = string.IsNullOrWhiteSpace(source.FileName)
                ? string.Empty
                : Path.Combine(_modelsDir, source.FileName);
            var partialPath = string.IsNullOrWhiteSpace(source.FileName)
                ? string.Empty
                : GetPartialDownloadPath(source.FileName);

            bool installed;
            bool active;
            long size;
            var hasPartialDownload = false;
            if (storeById.TryGetValue(source.Id, out var fromStore))
            {
                installed = string.Equals(fromStore.Health, "ok", StringComparison.OrdinalIgnoreCase)
                    && (!string.IsNullOrWhiteSpace(fromStore.DirectoryPath))
                    && (Directory.Exists(fromStore.DirectoryPath) || File.Exists(fromStore.DirectoryPath));
                active = installed && (
                    fromStore.IsDefault == true ||
                    (store?.ActiveModelId is not null && source.Id.Equals(store.ActiveModelId, StringComparison.OrdinalIgnoreCase))
                );
                size = (fromStore.SizeBytes ?? 0) > 0
                    ? fromStore.SizeBytes!.Value
                    : GetInstalledItemSize(source, localPath);
            }
            else
            {
                if (source.CanDownload)
                {
                    installed = !string.IsNullOrWhiteSpace(localPath) && File.Exists(localPath);
                    active = installed && NormalizePath(localPath) == activePath;
                    size = installed ? new FileInfo(localPath).Length : source.SizeBytes;
                    hasPartialDownload = !installed
                        && !string.IsNullOrWhiteSpace(partialPath)
                        && File.Exists(partialPath)
                        && new FileInfo(partialPath).Length > 0;
                }
                else if (string.Equals(source.RuntimeId, "server", StringComparison.OrdinalIgnoreCase))
                {
                    installed = IsRuntimeProfileInstalled(source.Id);
                    var runtimePath = NormalizePath(GetRuntimeProfileModelPath(source) ?? source.ExecutionModelRef);
                    var backendIsServer = string.Equals(backend, "server", StringComparison.OrdinalIgnoreCase);
                    active = installed && backendIsServer && runtimePath == activePath;
                    size = installed ? GetInstalledItemSize(source, localPath) : source.SizeBytes;
                }
                else
                {
                    installed = false;
                    active = false;
                    size = source.SizeBytes;
                }
            }

            if (!existing.TryGetValue(source.Id, out var vm))
            {
                vm = source.Clone();
            }

            vm.SyncFrom(source, installed, active, size);
            vm.HasPartialDownload = hasPartialDownload;
            vm.SuitabilityScore = EstimateSuitability(source, _lastHardwareScore, hasCudaHint, runtimePref);
            next.Add(vm);
        }

        _catalogModels.Clear();
        foreach (var item in next)
        {
            _catalogModels.Add(item);
        }

        _ = RefreshCatalogSizeHintsAsync();
        RefreshWelcomeSummary();
    }

    private static double EstimateSuitability(CatalogModelItem model, int hardwareScore, bool hasCuda, string runtimePref)
    {
        // Profile-aware score in [0..10] used for user-facing ordering.
        var speedW = hardwareScore >= 8 ? 0.35 : hardwareScore >= 5 ? 0.5 : 0.7;
        var accW = 1.0 - speedW;
        var score = model.Speed10 * speedW + model.Accuracy10 * accW;

        var isServer = string.Equals(model.RuntimeId, "server", StringComparison.OrdinalIgnoreCase);
        var isCloud = string.Equals(model.RuntimeId, "cloud", StringComparison.OrdinalIgnoreCase);

        if (isServer && !hasCuda) score -= 3.0;
        if (isServer && runtimePref == "force_cpu") score -= 2.0;
        if (isCloud) score -= 1.5; // local-first ordering
        if (runtimePref == "force_gpu" && hasCuda && (isServer || model.Accuracy10 >= 9)) score += 0.8;
        if (hardwareScore <= 4 && model.SizeBytes > 1024L * 1024 * 1024) score -= 1.5;

        return Math.Clamp(score, 0.0, 10.0);
    }

    private void LoadConfigFields()
    {
        var runtime = (GetTomlString("runtime", "preference") ?? "auto").Trim();
        RuntimeModeCombo.SelectedIndex = runtime switch { "force_gpu" => 1, "force_cpu" => 2, _ => 0 };
        RuntimeSummary.Text = runtime switch
        {
            "force_gpu" => "GPU forced. If unavailable, app falls back automatically.",
            "force_cpu" => "CPU forced. Stable but potentially slower.",
            _ => "Auto mode chooses best runtime by hardware diagnostics.",
        };

        var injection = (GetTomlString("injection", "method") ?? "direct").Trim();
        InjectionCombo.SelectedIndex = injection switch { "clipboard" => 1, "clipboard_enter" => 2, _ => 0 };

        StreamingToggle.IsOn = GetTomlBool("streaming", "enabled") ?? false;
        ChunkSecondsBox.Value = GetTomlUInt("streaming", "poll_interval") ?? 15;

        LlmToggle.IsOn = GetTomlBool("ollama", "enabled") ?? false;
        OllamaUrlBox.Text = GetTomlString("ollama", "url") ?? "http://localhost:11434";
        OllamaModelBox.Text = GetTomlString("ollama", "model") ?? "glm-4.7-flash";
        IdleMinutesBox.Value = GetTomlUInt("memory", "idle_unload_minutes") ?? 5;
        PostOverlayToggle.IsOn = GetTomlBool("ui", "show_post_transcription_overlay") ?? true;
        PostOverlaySecondsBox.Value = GetTomlUInt("ui", "post_transcription_overlay_seconds") ?? 3;
    }

    private void RefreshHardwareDiagnostics()
    {
        try
        {
            var cpuName = QueryWmi("Win32_Processor", "Name") ?? "Unknown CPU";
            var cpuCores = QueryWmiInt("Win32_Processor", "NumberOfCores");
            var ramGb = Math.Round(GetTotalRamGb(), 1);
            var gpuName = QueryWmi("Win32_VideoController", "Name") ?? "Unknown GPU";
            var gpuRamMb = QueryWmiULong("Win32_VideoController", "AdapterRAM") / (1024.0 * 1024.0);
            var hasCudaHint = gpuName.Contains("NVIDIA", StringComparison.OrdinalIgnoreCase);

            var score = 0;
            score += cpuCores >= 8 ? 4 : cpuCores >= 4 ? 2 : 1;
            score += ramGb >= 16 ? 3 : ramGb >= 8 ? 2 : 1;
            score += hasCudaHint ? 3 : gpuRamMb >= 2048 ? 2 : 1;
            score = Math.Clamp(score, 1, 10);

            var recommendation = score >= 8
                ? "Strong hardware: use Auto or Force GPU with high-accuracy models."
                : score >= 5
                    ? "Mid hardware: Auto mode recommended with base/small models."
                    : "Weak hardware: prefer CPU mode and tiny/base. Consider cloud fallback later.";

            _lastHardwareScore = score;
            HardwareScoreValue.Text = $"{score}/10";
            HardwareScoreValue.Foreground = new SolidColorBrush(score >= 8 ? Colors.LightGreen : score >= 5 ? Colors.Gold : Colors.OrangeRed);
            HardwareScoreHint.Text = "Hardware score";
            HardwareCpuText.Text = $"CPU: {cpuName} ({cpuCores} cores)";
            HardwareGpuText.Text = $"GPU: {gpuName} ({Math.Round(gpuRamMb / 1024.0, 1)} GB VRAM est.)";
            HardwareRamText.Text = $"RAM: {ramGb} GB";
            HardwareRecommendationText.Text = recommendation;
            RuntimeModelRecommendations.Text = BuildRuntimeModelRecommendations(score, hasCudaHint);
        }
        catch
        {
            HardwareScoreValue.Text = "N/A";
            HardwareScoreHint.Text = "Hardware score unavailable";
            HardwareCpuText.Text = "CPU: unavailable";
            HardwareGpuText.Text = "GPU: unavailable";
            HardwareRamText.Text = "RAM: unavailable";
            HardwareRecommendationText.Text = "Diagnostics are unavailable on this system.";
            RuntimeModelRecommendations.Text = "Recommendations unavailable until diagnostics are ready.";
        }
    }


    private string BuildRuntimeModelRecommendations(int score, bool hasCudaHint)
    {
        if (score >= 8 && hasCudaHint)
        {
            return "Recommended: large-v3-turbo or medium for local mode. Parakeet/Canary/Granite are available as server runtimes.";
        }
        if (score >= 8)
        {
            return "Recommended: large-v3-turbo or medium for local mode. GPU server runtimes require NVIDIA CUDA.";
        }
        if (score >= 5)
        {
            return "Recommended: base, small or medium. Keep Auto runtime for balanced speed and quality.";
        }
        return "Recommended: tiny or base in CPU mode. If latency is too high, use cloud profile.";
    }

    private void RefreshStorageCards()
    {
        ModelsPathText.Text = _modelsDir;
        ModelsSizeText.Text = $"Size: {FormatBytes(ComputeDirectorySize(_modelsDir))}";
        AudioPathText.Text = _audioHistoryDir;
        AudioSizeText.Text = $"Size: {FormatBytes(ComputeDirectorySize(_audioHistoryDir))}";
        TranscriptsPathText.Text = _transcriptsDir;
        TranscriptsSizeText.Text = $"Size: {FormatBytes(ComputeDirectorySize(_transcriptsDir))}";
    }

    private void OnNavChanged(NavigationView sender, NavigationViewSelectionChangedEventArgs args)
    {
        if (args.SelectedItem is not NavigationViewItem item || item.Tag is not string tag) return;

        WelcomePanel.Visibility = tag == "dashboard" ? Visibility.Visible : Visibility.Collapsed;
        ModelsPanel.Visibility = tag == "models" ? Visibility.Visible : Visibility.Collapsed;
        RuntimePanel.Visibility = tag == "runtime" ? Visibility.Visible : Visibility.Collapsed;
        DictationPanel.Visibility = tag == "dictation" ? Visibility.Visible : Visibility.Collapsed;
        HistoryPanel.Visibility = tag == "history" ? Visibility.Visible : Visibility.Collapsed;
        StoragePanel.Visibility = tag == "storage" ? Visibility.Visible : Visibility.Collapsed;
        AboutPanel.Visibility = tag == "about" ? Visibility.Visible : Visibility.Collapsed;

        PageTitle.Text = tag switch
        {
            "dashboard" => "Dashboard",
            "runtime" => "Runtime & Device",
            "dictation" => "Dictation",
            "history" => "History",
            "storage" => "Storage",
            "about" => "About",
            _ => "Models",
        };

        if (tag == "runtime" && !_runtimeDiagnosticsLoaded)
        {
            RefreshHardwareDiagnostics();
            LoadCatalog();
            _runtimeDiagnosticsLoaded = true;
        }

        if (tag == "history" && !_historyLoaded)
        {
            RefreshHistoryEntries();
            _historyLoaded = true;
        }
    }

    private void OnGoToModelsClick(object sender, RoutedEventArgs e) => SelectNavByTag("models");
    private void OnGoToRuntimeClick(object sender, RoutedEventArgs e) => SelectNavByTag("runtime");
    private void OnGoToDictationClick(object sender, RoutedEventArgs e) => SelectNavByTag("dictation");

    private void OnCatalogCardTapped(object sender, TappedRoutedEventArgs e)
    {
        if (sender is not FrameworkElement element || element.DataContext is not CatalogModelItem model) return;
        if (IsOriginalSourceInsideButton(e.OriginalSource)) return;
        if (!model.IsInstalled || model.IsActive) return;

        ActivateModel(model);
    }

    private void ActivateModel(CatalogModelItem model)
    {
        if (!model.IsInstalled) return;

        if (string.Equals(model.RuntimeId, "server", StringComparison.OrdinalIgnoreCase))
        {
            var runtimePath = GetRuntimeProfileModelPath(model);
            if (string.IsNullOrWhiteSpace(runtimePath))
            {
                model.DownloadStatusText = "Model files are missing. Download again.";
                return;
            }

            UpsertTomlValue("whisper", "backend", QuoteToml("server"));
            UpsertTomlValue("whisper", "model_path", QuoteToml(runtimePath));
            RuntimeSummary.Text = $"Selected {model.Name} (local).";
        }
        else
        {
            var localPath = Path.Combine(_modelsDir, model.FileName);
            UpsertTomlValue("whisper", "backend", QuoteToml("embedded"));
            UpsertTomlValue("whisper", "model_path", QuoteToml(localPath));
            RuntimeSummary.Text = $"Selected {model.Name} (local).";
        }

        SyncSharedModelStore();
        LoadCatalog();
    }

    private static bool IsOriginalSourceInsideButton(object? source)
    {
        var current = source as DependencyObject;
        while (current is not null)
        {
            if (current is Button) return true;
            current = VisualTreeHelper.GetParent(current);
        }
        return false;
    }


    private void OnCatalogExternalClick(object sender, RoutedEventArgs e)
    {
        if (sender is not Button btn || btn.Tag is not string id) return;
        var model = _catalogModels.FirstOrDefault(m => m.Id == id);
        if (model is null) return;

        if (!string.Equals(model.RuntimeId, "cloud", StringComparison.OrdinalIgnoreCase))
        {
            return;
        }

        try
        {
            UpsertTomlValue("runtime", "cloud_target", QuoteToml(model.ExecutionModelRef));
            RuntimeSummary.Text = "Cloud profile selected. Configure cloud credentials before use.";
        }
        catch (Exception ex)
        {
            model.DownloadStatusText = $"Action failed: {ex.Message}";
        }
    }

    private async void OnCatalogDownloadClick(object sender, RoutedEventArgs e)
    {
        if (sender is not Button btn || btn.Tag is not string id) return;
        var model = _catalogModels.FirstOrDefault(m => m.Id == id);
        if (model is null || model.IsInstalled || model.IsDownloading) return;

        if (string.Equals(model.RuntimeId, "server", StringComparison.OrdinalIgnoreCase))
        {
            await InstallRuntimeProfileAsync(model);
            LoadCatalog();
            return;
        }

        if (model.CanDownload)
        {
            await DownloadModelAsync(model);
        }
    }

    private void OnCatalogCancelClick(object sender, RoutedEventArgs e)
    {
        if (sender is not Button btn || btn.Tag is not string id) return;
        if (_downloadJobs.TryGetValue(id, out var cts))
        {
            cts.Cancel();
        }
    }

    private void OnCatalogDeleteClick(object sender, RoutedEventArgs e)
    {
        if (sender is not Button btn || btn.Tag is not string id) return;
        var model = _catalogModels.FirstOrDefault(m => m.Id == id);
        if (model is null || !model.IsInstalled) return;

        if (_downloadJobs.TryGetValue(model.Id, out var cts))
        {
            cts.Cancel();
            return;
        }

        try
        {
            var activePath = NormalizePath(GetTomlString("whisper", "model_path") ?? string.Empty);
            var backend = (GetTomlString("whisper", "backend") ?? string.Empty).Trim();
            var partialPath = string.IsNullOrWhiteSpace(model.FileName)
                ? string.Empty
                : GetPartialDownloadPath(model.FileName);

            bool deletingActive = false;
            if (string.Equals(model.RuntimeId, "server", StringComparison.OrdinalIgnoreCase))
            {
                var runtimePath = NormalizePath(GetRuntimeProfileModelPath(model) ?? string.Empty);
                deletingActive = string.Equals(backend, "server", StringComparison.OrdinalIgnoreCase) && runtimePath == activePath;

                var marker = GetRuntimeProfileMarkerPath(model.Id);
                if (File.Exists(marker)) File.Delete(marker);

                var runtimeModelDir = Path.Combine(_modelsDir, "runtime-models", model.Id);
                if (Directory.Exists(runtimeModelDir)) Directory.Delete(runtimeModelDir, true);
            }
            else
            {
                var localPath = Path.Combine(_modelsDir, model.FileName);
                deletingActive = NormalizePath(localPath) == activePath;
                if (File.Exists(localPath)) File.Delete(localPath);
            }

            if (!string.IsNullOrWhiteSpace(partialPath) && File.Exists(partialPath))
            {
                File.Delete(partialPath);
            }

            if (deletingActive)
            {
                LoadCatalog();
                var replacement = _catalogModels
                    .Where(m => m.IsInstalled && !m.Id.Equals(model.Id, StringComparison.OrdinalIgnoreCase))
                    .OrderBy(m => m.SizeBytes)
                    .FirstOrDefault();

                if (replacement is not null)
                {
                    ActivateModel(replacement);
                }
                else
                {
                    UpsertTomlValue("whisper", "model_path", QuoteToml(string.Empty));
                    UpsertTomlValue("whisper", "backend", QuoteToml("embedded"));
                }
            }

            SyncSharedModelStore();
            LoadCatalog();
        }
        catch
        {
            model.DownloadStatusText = "Delete failed.";
        }
    }

    private async Task DownloadModelAsync(CatalogModelItem model)
    {
        var cts = new CancellationTokenSource();
        if (!_downloadJobs.TryAdd(model.Id, cts)) return;

        var targetPath = Path.Combine(_modelsDir, model.FileName);
        var tempPath = GetPartialDownloadPath(model.FileName);
        var expectedBytes = model.ReportedSizeBytes ?? model.SizeBytes;
        if (!HasEnoughDiskSpace(targetPath, expectedBytes))
        {
            model.DownloadStatusText = $"Not enough disk space. Required: {FormatBytes(expectedBytes)}";
            _downloadJobs.Remove(model.Id);
            cts.Dispose();
            return;
        }

        model.IsDownloading = true;
        model.DownloadProgress = 0;
        model.DownloadStatusText = "Preparing download...";

        try
        {
            using var http = new HttpClient { Timeout = TimeSpan.FromMinutes(30) };
            long existingBytes = File.Exists(tempPath) ? new FileInfo(tempPath).Length : 0;
            using var request = new HttpRequestMessage(HttpMethod.Get, model.Url);
            if (existingBytes > 0)
            {
                request.Headers.Range = new RangeHeaderValue(existingBytes, null);
            }

            using var response = await http.SendAsync(request, HttpCompletionOption.ResponseHeadersRead, cts.Token);
            response.EnsureSuccessStatusCode();

            var isResuming = existingBytes > 0 && response.StatusCode == HttpStatusCode.PartialContent;
            if (existingBytes > 0 && !isResuming)
            {
                // Server ignored range; restart from scratch to avoid corrupt output.
                existingBytes = 0;
            }

            var contentLength = response.Content.Headers.ContentLength.GetValueOrDefault(model.SizeBytes);
            var total = isResuming ? existingBytes + contentLength : contentLength;
            if (total > 0) model.ReportedSizeBytes = total;
            long downloaded = existingBytes;
            var started = DateTime.UtcNow;
            var buffer = new byte[128 * 1024];

            await using (var source = await response.Content.ReadAsStreamAsync(cts.Token))
            await using (var dest = new FileStream(
                tempPath,
                isResuming ? FileMode.Append : FileMode.Create,
                FileAccess.Write,
                FileShare.None,
                128 * 1024,
                useAsync: true))
            {
                while (true)
                {
                    var read = await source.ReadAsync(buffer.AsMemory(0, buffer.Length), cts.Token);
                    if (read <= 0) break;

                    await dest.WriteAsync(buffer.AsMemory(0, read), cts.Token);
                    downloaded += read;

                    var progress = total > 0 ? (double)downloaded / total : 0;
                    model.DownloadProgress = Math.Clamp(progress * 100.0, 0.0, 100.0);

                    var elapsed = Math.Max((DateTime.UtcNow - started).TotalSeconds, 0.001);
                    var speed = downloaded / elapsed;
                    var eta = speed > 1 && total > downloaded ? TimeSpan.FromSeconds((total - downloaded) / speed) : TimeSpan.Zero;
                    model.DownloadStatusText = $"{model.DownloadProgress:F0}% | {FormatBytes(downloaded)} / {FormatBytes(total)} | {FormatBytes((long)speed)}/s | ETA {eta:mm\\:ss}";
                }

                await dest.FlushAsync(cts.Token);
            }
            await ReplaceFileWithRetriesAsync(tempPath, targetPath, cts.Token);

            model.IsInstalled = true;
            model.InstalledBytes = new FileInfo(targetPath).Length;
            model.HasPartialDownload = false;

            var activePath = GetTomlString("whisper", "model_path")?.Trim() ?? string.Empty;
            if (string.IsNullOrWhiteSpace(activePath))
            {
                UpsertTomlValue("whisper", "model_path", QuoteToml(targetPath));
                foreach (var item in _catalogModels)
                    {
                        item.IsInstalled = item.Id.Equals(model.Id, StringComparison.OrdinalIgnoreCase) || item.IsInstalled;
                        item.IsActive = item.Id.Equals(model.Id, StringComparison.OrdinalIgnoreCase);
                    }
                    LoadCatalog();
            }

            model.DownloadStatusText = string.Empty;
            model.DownloadProgress = 0;
        }
        catch (OperationCanceledException)
        {
            model.DownloadStatusText = "Download paused. Click Resume.";
            model.HasPartialDownload = File.Exists(tempPath) && new FileInfo(tempPath).Length > 0;
        }
        catch (Exception ex)
        {
            model.DownloadStatusText = $"Download failed: {ex.Message}";
            model.HasPartialDownload = File.Exists(tempPath) && new FileInfo(tempPath).Length > 0;
        }
        finally
        {
            model.IsDownloading = false;

            if (_downloadJobs.TryGetValue(model.Id, out var job) && ReferenceEquals(job, cts))
            {
                _downloadJobs.Remove(model.Id);
            }
            cts.Dispose();

            SyncSharedModelStore();
            LoadCatalog();
        }
    }

    private string GetPartialDownloadPath(string fileName)
        => Path.Combine(_modelsDir, fileName + ".partial");

    private static bool HasEnoughDiskSpace(string targetPath, long requiredBytes)
    {
        try
        {
            var root = Path.GetPathRoot(targetPath);
            if (string.IsNullOrWhiteSpace(root)) return true;
            var drive = new DriveInfo(root);
            // Keep small reserve for temp and metadata writes.
            var requiredWithReserve = (long)(requiredBytes * 1.1) + (128L * 1024 * 1024);
            return drive.AvailableFreeSpace > requiredWithReserve;
        }
        catch
        {
            return true;
        }
    }


    private async Task RefreshCatalogSizeHintsAsync()
    {
        foreach (var model in _catalogModels)
        {
            if (!model.CanDownload || model.IsInstalled || string.IsNullOrWhiteSpace(model.Url)) continue;
            if (!_sizeProbeQueued.Add(model.Id)) continue;

            var remoteLength = await TryGetRemoteContentLengthAsync(model.Url);
            if (remoteLength.HasValue && remoteLength.Value > 0)
            {
                model.ReportedSizeBytes = remoteLength.Value;
            }
            else
            {
                _sizeProbeQueued.Remove(model.Id);
            }
        }
    }

    private static async Task<long?> TryGetRemoteContentLengthAsync(string url)
    {
        try
        {
            using var http = new HttpClient { Timeout = TimeSpan.FromSeconds(20) };
            using var headReq = new HttpRequestMessage(HttpMethod.Head, url);
            using var headResp = await http.SendAsync(headReq, HttpCompletionOption.ResponseHeadersRead);
            if (headResp.IsSuccessStatusCode && headResp.Content.Headers.ContentLength.HasValue)
            {
                return headResp.Content.Headers.ContentLength.Value;
            }

            using var getReq = new HttpRequestMessage(HttpMethod.Get, url);
            getReq.Headers.Range = new RangeHeaderValue(0, 0);
            using var getResp = await http.SendAsync(getReq, HttpCompletionOption.ResponseHeadersRead);
            if (getResp.IsSuccessStatusCode && getResp.Content.Headers.ContentLength.HasValue)
            {
                return getResp.Content.Headers.ContentLength.Value;
            }
        }
        catch { }

        return null;
    }

    private static async Task ReplaceFileWithRetriesAsync(string sourcePath, string targetPath, CancellationToken cancellationToken)
    {
        await WaitForFileReadyAsync(sourcePath, cancellationToken);

        const int maxAttempts = 120;
        Exception? lastError = null;

        for (var attempt = 1; attempt <= maxAttempts; attempt++)
        {
            cancellationToken.ThrowIfCancellationRequested();
            try
            {
                if (File.Exists(targetPath))
                {
                    try
                    {
                        var attrs = File.GetAttributes(targetPath);
                        if ((attrs & FileAttributes.ReadOnly) != 0)
                        {
                            File.SetAttributes(targetPath, attrs & ~FileAttributes.ReadOnly);
                        }
                    }
                    catch { }

                    File.Delete(targetPath);
                }

                File.Move(sourcePath, targetPath);
                return;
            }
            catch (IOException ex) when (attempt < maxAttempts)
            {
                lastError = ex;
                await Task.Delay(TimeSpan.FromMilliseconds(Math.Min(500 * attempt, 2000)), cancellationToken);
            }
            catch (UnauthorizedAccessException ex) when (attempt < maxAttempts)
            {
                lastError = ex;
                await Task.Delay(TimeSpan.FromMilliseconds(Math.Min(500 * attempt, 2000)), cancellationToken);
            }
        }

        throw new IOException(
            $"Failed to finalize download after {maxAttempts} attempts. Source={sourcePath}, Target={targetPath}",
            lastError
        );
    }

    private static async Task WaitForFileReadyAsync(string path, CancellationToken cancellationToken)
    {
        const int maxAttempts = 120;

        for (var attempt = 1; attempt <= maxAttempts; attempt++)
        {
            cancellationToken.ThrowIfCancellationRequested();
            try
            {
                await using var stream = new FileStream(
                    path,
                    FileMode.Open,
                    FileAccess.Read,
                    FileShare.Read,
                    1,
                    useAsync: true
                );
                return;
            }
            catch (IOException) when (attempt < maxAttempts)
            {
                await Task.Delay(TimeSpan.FromMilliseconds(Math.Min(500 * attempt, 2000)), cancellationToken);
            }
            catch (UnauthorizedAccessException) when (attempt < maxAttempts)
            {
                await Task.Delay(TimeSpan.FromMilliseconds(Math.Min(500 * attempt, 2000)), cancellationToken);
            }
        }

        throw new IOException($"Downloaded file is not ready for finalization: {path}");
    }
    private string GetRuntimeProfileMarkerPath(string modelId)
        => Path.Combine(RuntimeProfilesDir, modelId + ".json");

    private bool IsRuntimeProfileInstalled(string modelId)
    {
        var marker = GetRuntimeProfileMarkerPath(modelId);
        if (File.Exists(marker)) return true;

        var runtimeModelDir = Path.Combine(_modelsDir, "runtime-models", modelId);
        if (!Directory.Exists(runtimeModelDir)) return false;

        try
        {
            return Directory.EnumerateFiles(runtimeModelDir, "*", SearchOption.AllDirectories).Any();
        }
        catch
        {
            return false;
        }
    }

    private string? GetRuntimeProfileModelPath(CatalogModelItem model)
    {
        try
        {
            var marker = GetRuntimeProfileMarkerPath(model.Id);
            if (!File.Exists(marker))
            {
                var runtimeModelDir = Path.Combine(_modelsDir, "runtime-models", model.Id);
                return Directory.Exists(runtimeModelDir) ? runtimeModelDir : null;
            }

            var json = File.ReadAllText(marker);
            var state = JsonSerializer.Deserialize<RuntimeProfileState>(json);
            return state?.LocalModelPath;
        }
        catch
        {
            var runtimeModelDir = Path.Combine(_modelsDir, "runtime-models", model.Id);
            return Directory.Exists(runtimeModelDir) ? runtimeModelDir : null;
        }
    }

    private long GetInstalledItemSize(CatalogModelItem source, string localPath)
    {
        if (source.CanDownload)
        {
            return File.Exists(localPath) ? new FileInfo(localPath).Length : source.SizeBytes;
        }

        var runtimePath = GetRuntimeProfileModelPath(source);
        if (!string.IsNullOrWhiteSpace(runtimePath) && Directory.Exists(runtimePath))
        {
            return ComputeDirectorySize(runtimePath);
        }

        return source.SizeBytes;
    }

    private async Task InstallRuntimeProfileAsync(CatalogModelItem model)
    {
        model.IsDownloading = true;
        model.DownloadProgress = 0;
        model.DownloadStatusText = "Preparing runtime installer...";

        var runtimeRoot = Path.Combine(_modelsDir, "runtimes", "python-asr");
        var venvDir = Path.Combine(runtimeRoot, "venv");
        var venvPython = Path.Combine(venvDir, "Scripts", "python.exe");
        var modelRoot = Path.Combine(_modelsDir, "runtime-models", model.Id);

        Directory.CreateDirectory(RuntimeProfilesDir);
        Directory.CreateDirectory(runtimeRoot);
        Directory.CreateDirectory(modelRoot);

        var hasNvidia = (HardwareGpuText.Text ?? string.Empty).Contains("NVIDIA", StringComparison.OrdinalIgnoreCase);

        var hostPython = "python";
        if (!File.Exists(venvPython))
        {
            model.DownloadProgress = 8;
            model.DownloadStatusText = "Creating runtime environment...";
            await RunProcessCheckedAsync(hostPython, $"-m venv \"{venvDir}\"", runtimeRoot);
        }

        model.DownloadProgress = 20;
        model.DownloadStatusText = "Installing runtime dependencies...";
        await RunProcessCheckedAsync(venvPython, "-m pip install --upgrade pip", runtimeRoot);

        var reqPath = FindWhisperServerRequirementsPath();
        if (!string.IsNullOrWhiteSpace(reqPath) && File.Exists(reqPath))
        {
            await RunProcessCheckedAsync(venvPython, $"-m pip install -r \"{reqPath}\"", runtimeRoot);
        }
        else
        {
            await RunProcessCheckedAsync(venvPython, "-m pip install flask faster-whisper transformers accelerate huggingface_hub", runtimeRoot);
        }

        model.DownloadProgress = 45;
        model.DownloadStatusText = "Installing torch runtime...";
        var torchIndex = hasNvidia
            ? "https://download.pytorch.org/whl/cu121"
            : "https://download.pytorch.org/whl/cpu";
        await RunProcessCheckedAsync(venvPython, $"-m pip install torch --index-url {torchIndex}", runtimeRoot);

        model.DownloadProgress = 62;
        model.DownloadStatusText = "Downloading model weights...";
        var installerScriptPath = Path.Combine(runtimeRoot, "install_runtime_profile.py");
        File.WriteAllText(installerScriptPath,
            "import sys\r\n" +
            "from huggingface_hub import snapshot_download\r\n" +
            "repo_id = sys.argv[1]\r\n" +
            "local_dir = sys.argv[2]\r\n" +
            "snapshot_download(repo_id=repo_id, local_dir=local_dir, local_dir_use_symlinks=False)\r\n");
        await RunProcessCheckedAsync(
            venvPython,
            $"\"{installerScriptPath}\" \"{model.ExecutionModelRef}\" \"{modelRoot}\"",
            runtimeRoot);

        model.DownloadProgress = 90;
        model.DownloadStatusText = "Saving runtime profile...";

        var state = new RuntimeProfileState
        {
            ModelId = model.Id,
            ExecutionModelRef = model.ExecutionModelRef,
            LocalModelPath = modelRoot,
            PythonPath = venvPython,
            InstalledAtUtc = DateTime.UtcNow,
        };

        var markerPath = GetRuntimeProfileMarkerPath(model.Id);
        File.WriteAllText(markerPath, JsonSerializer.Serialize(state, new JsonSerializerOptions { WriteIndented = true }));

        model.DownloadProgress = 100;
        model.DownloadStatusText = "Runtime installed.";
        model.IsInstalled = true;
        SyncSharedModelStore();
    }

    private static string? FindWhisperServerRequirementsPath()
    {
        try
        {
            var baseDir = AppContext.BaseDirectory;
            var current = new DirectoryInfo(baseDir);
            while (current is not null)
            {
                var candidate = Path.Combine(current.FullName, "shared", "whisper-server", "requirements.txt");
                if (File.Exists(candidate)) return candidate;
                current = current.Parent;
            }
        }
        catch { }
        return null;
    }

    private static async Task RunProcessCheckedAsync(string fileName, string arguments, string workingDirectory)
    {
        var start = new ProcessStartInfo
        {
            FileName = fileName,
            Arguments = arguments,
            WorkingDirectory = workingDirectory,
            UseShellExecute = false,
            CreateNoWindow = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
        };

        using var proc = new Process { StartInfo = start };
        proc.Start();
        var stdOut = await proc.StandardOutput.ReadToEndAsync();
        var stdErr = await proc.StandardError.ReadToEndAsync();
        await proc.WaitForExitAsync();

        if (proc.ExitCode != 0)
        {
            var details = string.IsNullOrWhiteSpace(stdErr) ? stdOut : stdErr;
            throw new InvalidOperationException($"Process failed ({fileName} {arguments}): {details}");
        }
    }
    private void OnRuntimeChanged(object sender, SelectionChangedEventArgs e)
    {
        if (!_uiReady) return;
        var selected = (RuntimeModeCombo.SelectedItem as ComboBoxItem)?.Tag as string ?? "auto";
        UpsertTomlValue("runtime", "preference", QuoteToml(selected));

        RuntimeSummary.Text = selected switch
        {
            "force_gpu" => "GPU forced. Use only if you know this machine has compatible acceleration.",
            "force_cpu" => "CPU forced. Reliable mode, possibly slower for large models.",
            _ => "Auto mode selected: recommendation follows hardware diagnostics.",
        };
        RefreshWelcomeSummary();
        RefreshDictationGuidance();
    }

    private void OnInjectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (!_uiReady) return;
        var selected = (InjectionCombo.SelectedItem as ComboBoxItem)?.Tag as string ?? "direct";
        UpsertTomlValue("injection", "method", QuoteToml(selected));
        RefreshDictationGuidance();
    }

    private void OnStreamingToggled(object sender, RoutedEventArgs e)
    {
        if (!_uiReady) return;
        UpsertTomlValue("streaming", "enabled", StreamingToggle.IsOn ? "true" : "false");
        RefreshWelcomeSummary();
        RefreshDictationGuidance();
    }

    private void OnChunkValueChanged(NumberBox sender, NumberBoxValueChangedEventArgs args)
    {
        if (!_uiReady) return;
        var value = (uint)Math.Clamp((int)Math.Round(sender.Value), 3, 60);
        UpsertTomlValue("streaming", "poll_interval", value.ToString(CultureInfo.InvariantCulture));
        RefreshDictationGuidance();
    }

    private void OnLlmToggled(object sender, RoutedEventArgs e)
    {
        if (!_uiReady) return;
        UpsertTomlValue("ollama", "enabled", LlmToggle.IsOn ? "true" : "false");
        RefreshDictationGuidance();
    }

    private void OnPostOverlayToggled(object sender, RoutedEventArgs e)
    {
        if (!_uiReady) return;
        UpsertTomlValue("ui", "show_post_transcription_overlay", PostOverlayToggle.IsOn ? "true" : "false");
        RefreshDictationGuidance();
    }

    private void OnPostOverlaySecondsChanged(NumberBox sender, NumberBoxValueChangedEventArgs args)
    {
        if (!_uiReady) return;
        var value = (uint)Math.Clamp((int)Math.Round(sender.Value), 1, 15);
        UpsertTomlValue("ui", "post_transcription_overlay_seconds", value.ToString(CultureInfo.InvariantCulture));
        RefreshDictationGuidance();
    }

    private void OnConfigFieldLostFocus(object sender, RoutedEventArgs e)
    {
        if (!_uiReady) return;
        UpsertTomlValue("ollama", "url", QuoteToml(OllamaUrlBox.Text.Trim()));
        UpsertTomlValue("ollama", "model", QuoteToml(OllamaModelBox.Text.Trim()));
    }

    private void OnIdleValueChanged(NumberBox sender, NumberBoxValueChangedEventArgs args)
    {
        if (!_uiReady) return;
        var value = (uint)Math.Clamp((int)Math.Round(sender.Value), 1, 240);
        UpsertTomlValue("memory", "idle_unload_minutes", value.ToString(CultureInfo.InvariantCulture));
    }

    private async void OnChangeModelsFolder(object sender, RoutedEventArgs e)
    {
        var selected = await PickFolderAsync(_modelsDir);
        if (string.IsNullOrWhiteSpace(selected)) return;

        _modelsDir = selected;
        UpsertTomlValue("whisper", "models_dir", QuoteToml(_modelsDir));
        RefreshAll();
    }

    private async void OnChangeAudioFolder(object sender, RoutedEventArgs e)
    {
        var selected = await PickFolderAsync(_audioHistoryDir);
        if (string.IsNullOrWhiteSpace(selected)) return;

        _audioHistoryDir = selected;
        UpsertTomlValue("storage", "audio_history_dir", QuoteToml(_audioHistoryDir));
        RefreshStorageCards();
        RefreshHistoryEntries();
    }

    private async void OnChangeTranscriptsFolder(object sender, RoutedEventArgs e)
    {
        var selected = await PickFolderAsync(_transcriptsDir);
        if (string.IsNullOrWhiteSpace(selected)) return;

        _transcriptsDir = selected;
        UpsertTomlValue("storage", "transcripts_dir", QuoteToml(_transcriptsDir));
        RefreshStorageCards();
        RefreshHistoryEntries();
    }

    private void OnOpenModelsFolder(object sender, RoutedEventArgs e) => OpenPath(_modelsDir);
    private void OnOpenAudioFolder(object sender, RoutedEventArgs e) => OpenPath(_audioHistoryDir);
    private void OnOpenTranscriptsFolder(object sender, RoutedEventArgs e) => OpenPath(_transcriptsDir);
    private void OnClose(object sender, RoutedEventArgs e) => Close();

    private void OnHistoryRefreshClick(object sender, RoutedEventArgs e) => RefreshHistoryEntries();

    private void OnHistorySelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        _selectedHistory = HistoryList.SelectedItem as HistoryEntryItem;
        if (_selectedHistory is null)
        {
            HistoryDetailTitle.Text = "Select a history item";
            HistoryDetailMeta.Text = string.Empty;
            HistoryDetailText.Text = string.Empty;
            return;
        }

        HistoryDetailTitle.Text = _selectedHistory.Title;
        HistoryDetailMeta.Text = $"Audio: {_selectedHistory.AudioPath}\nTranscript: {_selectedHistory.TextPath}";
        try
        {
            HistoryDetailText.Text = File.Exists(_selectedHistory.TextPath)
                ? File.ReadAllText(_selectedHistory.TextPath)
                : "(Transcript file is missing)";
        }
        catch (Exception ex)
        {
            HistoryDetailText.Text = $"Failed to open transcript: {ex.Message}";
        }
    }

    private void OnHistoryCopyClick(object sender, RoutedEventArgs e)
    {
        if (_selectedHistory is null || !File.Exists(_selectedHistory.TextPath)) return;
        try
        {
            var text = File.ReadAllText(_selectedHistory.TextPath);
            var data = new Windows.ApplicationModel.DataTransfer.DataPackage();
            data.SetText(text);
            Windows.ApplicationModel.DataTransfer.Clipboard.SetContent(data);
        }
        catch { }
    }

    private void OnHistoryOpenAudioClick(object sender, RoutedEventArgs e)
    {
        if (_selectedHistory is null) return;
        OpenPath(_selectedHistory.AudioPath);
    }

    private void OnHistoryOpenTranscriptClick(object sender, RoutedEventArgs e)
    {
        if (_selectedHistory is null) return;
        OpenPath(_selectedHistory.TextPath);
    }

    private void OnHistoryDeleteClick(object sender, RoutedEventArgs e)
    {
        if (_selectedHistory is null) return;
        try
        {
            if (File.Exists(_selectedHistory.TextPath)) File.Delete(_selectedHistory.TextPath);
            if (!string.IsNullOrWhiteSpace(_selectedHistory.MetaPath) && File.Exists(_selectedHistory.MetaPath))
            {
                File.Delete(_selectedHistory.MetaPath);
            }
            if (File.Exists(_selectedHistory.AudioPath)) File.Delete(_selectedHistory.AudioPath);
        }
        catch { }

        _selectedHistory = null;
        RefreshHistoryEntries();
    }

    private void RefreshHistoryEntries()
    {
        Directory.CreateDirectory(_audioHistoryDir);
        Directory.CreateDirectory(_transcriptsDir);

        var items = new List<HistoryEntryItem>();
        foreach (var textPath in Directory.EnumerateFiles(_transcriptsDir, "*.txt", SearchOption.AllDirectories))
        {
            var file = new FileInfo(textPath);
            var id = Path.GetFileNameWithoutExtension(textPath);
            var day = file.Directory?.Name ?? string.Empty;
            var audioPath = Path.Combine(_audioHistoryDir, day, id + ".wav");
            var metaPath = Path.Combine(_transcriptsDir, day, id + ".json");

            string preview;
            try
            {
                var content = File.ReadAllText(textPath).Replace('\n', ' ').Trim();
                preview = content.Length > 96 ? content[..96] + "..." : content;
            }
            catch
            {
                preview = "(Failed to read transcript)";
            }

            var title = $"{file.LastWriteTime:yyyy-MM-dd HH:mm} · {id}";
            items.Add(new HistoryEntryItem
            {
                Id = id,
                Title = title,
                Preview = preview,
                AudioPath = audioPath,
                TextPath = textPath,
                MetaPath = metaPath,
                SortKey = file.LastWriteTimeUtc
            });
        }

        // Compatibility fallback: legacy flows may keep .txt next to audio files.
        if (items.Count == 0)
        {
            foreach (var textPath in Directory.EnumerateFiles(_audioHistoryDir, "*.txt", SearchOption.AllDirectories))
            {
                var file = new FileInfo(textPath);
                var id = Path.GetFileNameWithoutExtension(textPath);
                var day = file.Directory?.Name ?? string.Empty;
                var audioPath = Path.Combine(_audioHistoryDir, day, id + ".wav");
                var metaPath = Path.Combine(_transcriptsDir, day, id + ".json");

                string preview;
                try
                {
                    var content = File.ReadAllText(textPath).Replace('\n', ' ').Trim();
                    preview = content.Length > 96 ? content[..96] + "..." : content;
                }
                catch
                {
                    preview = "(Failed to read transcript)";
                }

                items.Add(new HistoryEntryItem
                {
                    Id = id,
                    Title = $"{file.LastWriteTime:yyyy-MM-dd HH:mm} · {id}",
                    Preview = preview,
                    AudioPath = audioPath,
                    TextPath = textPath,
                    MetaPath = metaPath,
                    SortKey = file.LastWriteTimeUtc
                });
            }
        }

        _historyEntries.Clear();
        foreach (var item in items.OrderByDescending(i => i.SortKey).Take(250))
        {
            _historyEntries.Add(item);
        }

        if (_historyEntries.Count == 0)
        {
            HistoryDetailTitle.Text = "No history entries yet";
            HistoryDetailMeta.Text = "Start dictation to populate audio/text history.";
            HistoryDetailText.Text = string.Empty;
            return;
        }

        if (_selectedHistory is not null)
        {
            var restored = _historyEntries.FirstOrDefault(x => x.Id == _selectedHistory.Id);
            if (restored is not null)
            {
                HistoryList.SelectedItem = restored;
            }
        }
    }

    private void LoadCorrectionsConfigFields()
    {
        CorrectionsToggle.IsOn = GetTomlBool("corrections", "enabled") ?? true;
        CorrectionsAutoLearnToggle.IsOn = GetTomlBool("corrections", "auto_learn") ?? true;
        CorrectionsMinRepeatsBox.Value = GetTomlUInt("corrections", "min_auto_learn_repeats") ?? 3;
    }

    private void OnCorrectionsToggled(object sender, RoutedEventArgs e)
    {
        if (!_uiReady) return;
        UpsertTomlValue("corrections", "enabled", CorrectionsToggle.IsOn ? "true" : "false");
    }

    private void OnCorrectionsAutoLearnToggled(object sender, RoutedEventArgs e)
    {
        if (!_uiReady) return;
        UpsertTomlValue("corrections", "auto_learn", CorrectionsAutoLearnToggle.IsOn ? "true" : "false");
    }

    private void OnCorrectionsMinRepeatsChanged(NumberBox sender, NumberBoxValueChangedEventArgs args)
    {
        if (!_uiReady) return;
        var value = (uint)Math.Clamp((int)Math.Round(sender.Value), 2, 20);
        UpsertTomlValue("corrections", "min_auto_learn_repeats", value.ToString(CultureInfo.InvariantCulture));
    }

    private void OnAddCorrectionClick(object sender, RoutedEventArgs e)
    {
        var from = CorrectionFromBox.Text.Trim();
        var to = CorrectionToBox.Text.Trim();
        if (string.IsNullOrWhiteSpace(from) || string.IsNullOrWhiteSpace(to)) return;

        var doc = LoadCorrectionsDoc();
        var existing = doc.Entries.FirstOrDefault(e =>
            string.Equals(e.From, from, StringComparison.OrdinalIgnoreCase));
        if (existing is null)
        {
            doc.Entries.Add(new CorrectionsDocEntry { From = from, To = to, Hits = 0 });
        }
        else
        {
            existing.To = to;
        }
        SaveCorrectionsDoc(doc);
        CorrectionFromBox.Text = string.Empty;
        CorrectionToBox.Text = string.Empty;
        RefreshCorrectionsList();
    }

    private void OnDeleteCorrectionClick(object sender, RoutedEventArgs e)
    {
        if (sender is not Button btn || btn.Tag is not string from) return;
        var doc = LoadCorrectionsDoc();
        doc.Entries = doc.Entries
            .Where(e => !string.Equals(e.From, from, StringComparison.OrdinalIgnoreCase))
            .ToList();
        SaveCorrectionsDoc(doc);
        RefreshCorrectionsList();
    }

    private void RefreshCorrectionsList()
    {
        var doc = LoadCorrectionsDoc();
        _correctionEntries.Clear();
        foreach (var entry in doc.Entries.OrderBy(e => e.From, StringComparer.OrdinalIgnoreCase))
        {
            _correctionEntries.Add(new CorrectionEntryItem
            {
                From = entry.From,
                Label = $"{entry.From} → {entry.To} (hits: {entry.Hits})"
            });
        }
    }

    private string GetCorrectionsPath()
    {
        var configured = (GetTomlString("corrections", "dictionary_path") ?? string.Empty).Trim();
        if (!string.IsNullOrWhiteSpace(configured)) return configured;
        return Path.Combine(_modelsDir, "shared_corrections.v1.json");
    }

    private CorrectionsDocModel LoadCorrectionsDoc()
    {
        var path = GetCorrectionsPath();
        try
        {
            if (!File.Exists(path))
            {
                var empty = new CorrectionsDocModel
                {
                    UpdatedAt = DateTimeOffset.UtcNow.ToString("O"),
                    Entries = new List<CorrectionsDocEntry>()
                };
                SaveCorrectionsDoc(empty);
                return empty;
            }
            var raw = File.ReadAllText(path);
            return JsonSerializer.Deserialize<CorrectionsDocModel>(raw, new JsonSerializerOptions
            {
                PropertyNameCaseInsensitive = true
            }) ?? new CorrectionsDocModel();
        }
        catch
        {
            return new CorrectionsDocModel();
        }
    }

    private void SaveCorrectionsDoc(CorrectionsDocModel doc)
    {
        var path = GetCorrectionsPath();
        var parent = Path.GetDirectoryName(path);
        if (!string.IsNullOrWhiteSpace(parent)) Directory.CreateDirectory(parent);

        doc.SchemaVersion = "dictator_corrections.v1";
        doc.UpdatedAt = DateTimeOffset.UtcNow.ToString("O");
        var json = JsonSerializer.Serialize(doc, new JsonSerializerOptions
        {
            WriteIndented = true,
            DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull
        });
        File.WriteAllText(path, json);
    }

    private async Task<string?> PickFolderAsync(string preferredPath)
    {
        try
        {
            var picker = new Windows.Storage.Pickers.FolderPicker();
            picker.FileTypeFilter.Add("*");
            var hwnd = WinRT.Interop.WindowNative.GetWindowHandle(this);
            WinRT.Interop.InitializeWithWindow.Initialize(picker, hwnd);
            picker.SuggestedStartLocation = Directory.Exists(preferredPath)
                ? Windows.Storage.Pickers.PickerLocationId.ComputerFolder
                : Windows.Storage.Pickers.PickerLocationId.DocumentsLibrary;
            var selected = await picker.PickSingleFolderAsync();
            return selected?.Path;
        }
        catch { return null; }
    }

    private void OpenPath(string path)
    {
        try
        {
            if (File.Exists(path))
            {
                Process.Start(new ProcessStartInfo { FileName = path, UseShellExecute = true });
                return;
            }

            Directory.CreateDirectory(path);
            Process.Start(new ProcessStartInfo { FileName = path, UseShellExecute = true });
        }
        catch { }
    }

    private string? GetTomlString(string section, string key)
    {
        var raw = GetTomlRawValue(section, key);
        if (raw is null) return null;
        raw = raw.Trim();
        if (raw.StartsWith('"') && raw.EndsWith('"') && raw.Length >= 2)
            return raw[1..^1].Replace("\\\\", "\\").Replace("\\\"", "\"");
        if (raw.StartsWith('\'') && raw.EndsWith('\'') && raw.Length >= 2)
            return raw[1..^1];
        return raw;
    }

    private bool? GetTomlBool(string section, string key)
    {
        var raw = GetTomlRawValue(section, key)?.Trim();
        if (raw is null) return null;
        if (raw.Equals("true", StringComparison.OrdinalIgnoreCase)) return true;
        if (raw.Equals("false", StringComparison.OrdinalIgnoreCase)) return false;
        return null;
    }

    private uint? GetTomlUInt(string section, string key)
    {
        var raw = GetTomlRawValue(section, key)?.Trim();
        if (raw is null) return null;
        return uint.TryParse(raw, NumberStyles.Integer, CultureInfo.InvariantCulture, out var value) ? value : null;
    }

    private string? GetTomlRawValue(string section, string key)
    {
        if (!File.Exists(_configPath)) return null;
        var lines = File.ReadAllLines(_configPath);
        var inSection = false;

        foreach (var sourceLine in lines)
        {
            var line = sourceLine.Trim();
            if (line.StartsWith("#") || line.Length == 0) continue;
            if (line.StartsWith('[') && line.EndsWith(']')) { inSection = line.Equals($"[{section}]", StringComparison.OrdinalIgnoreCase); continue; }
            if (!inSection) continue;
            if (!line.StartsWith(key + " ", StringComparison.OrdinalIgnoreCase) && !line.StartsWith(key + "=", StringComparison.OrdinalIgnoreCase)) continue;
            var idx = line.IndexOf('=');
            if (idx < 0) continue;
            return line[(idx + 1)..].Trim();
        }

        return null;
    }

    private void UpsertTomlValue(string section, string key, string valueLiteral)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(_configPath) ?? ".");
        var lines = File.Exists(_configPath) ? File.ReadAllLines(_configPath).ToList() : new List<string>();

        var sectionHeader = $"[{section}]";
        var sectionStart = lines.FindIndex(l => l.Trim().Equals(sectionHeader, StringComparison.OrdinalIgnoreCase));
        if (sectionStart < 0)
        {
            if (lines.Count > 0 && lines[^1].Length != 0) lines.Add(string.Empty);
            lines.Add(sectionHeader);
            lines.Add($"{key} = {valueLiteral}");
            File.WriteAllLines(_configPath, lines);
            return;
        }

        var sectionEnd = lines.Count;
        for (int i = sectionStart + 1; i < lines.Count; i++)
        {
            var t = lines[i].Trim();
            if (t.StartsWith('[') && t.EndsWith(']')) { sectionEnd = i; break; }
        }

        for (int i = sectionStart + 1; i < sectionEnd; i++)
        {
            var t = lines[i].TrimStart();
            if (!t.StartsWith(key, StringComparison.OrdinalIgnoreCase)) continue;
            var eq = t.IndexOf('=');
            if (eq < 0) continue;
            var head = t[..eq].Trim();
            if (!head.Equals(key, StringComparison.OrdinalIgnoreCase)) continue;
            var prefixLen = lines[i].Length - t.Length;
            var prefix = prefixLen > 0 ? lines[i][..prefixLen] : string.Empty;
            lines[i] = $"{prefix}{key} = {valueLiteral}";
            File.WriteAllLines(_configPath, lines);
            return;
        }

        lines.Insert(sectionEnd, $"{key} = {valueLiteral}");
        File.WriteAllLines(_configPath, lines);
    }

    private static string QuoteToml(string value)
    {
        var safe = value.Replace("\\", "\\\\").Replace("\"", "\\\"");
        return $"\"{safe}\"";
    }

    private static string NormalizePath(string path) => path.Replace('/', '\\').Trim().ToLowerInvariant();

    private SharedModelStoreDoc? LoadStoreSnapshot()
    {
        try
        {
            if (!File.Exists(_storePath)) return null;
            var raw = File.ReadAllText(_storePath);
            return JsonSerializer.Deserialize<SharedModelStoreDoc>(raw, new JsonSerializerOptions
            {
                PropertyNameCaseInsensitive = true
            });
        }
        catch
        {
            return null;
        }
    }

    private void SyncSharedModelStore()
    {
        try
        {
            var backend = (GetTomlString("whisper", "backend") ?? "embedded").Trim();
            var activePath = NormalizePath(GetTomlString("whisper", "model_path") ?? string.Empty);
            var serverRuntimeRoot = Path.Combine(_modelsDir, "runtimes", "python-asr");
            var existingStore = LoadStoreSnapshot() ?? new SharedModelStoreDoc();
            var managedRuntimeIds = new HashSet<string>(StringComparer.OrdinalIgnoreCase)
            {
                "embedded_whisper_rs",
                "server_python_asr"
            };

            var store = new SharedModelStoreDoc
            {
                SchemaVersion = "shared_model_store.v1",
                StoreVersion = Math.Max(existingStore.StoreVersion, 1),
                ModelsRootPath = _modelsDir,
                ActiveRuntimeId = string.Equals(backend, "server", StringComparison.OrdinalIgnoreCase)
                    ? "server_python_asr"
                    : "embedded_whisper_rs",
                ActiveModelId = null,
                UpdatedAt = DateTimeOffset.UtcNow.ToString("O"),
                UpdatedBy = "dictator",
                InstalledRuntimes = existingStore.InstalledRuntimes
                    .Where(r => !managedRuntimeIds.Contains(r.Id))
                    .ToList(),
                InstalledModels = existingStore.InstalledModels
                    .Where(m => !managedRuntimeIds.Contains(m.RuntimeId))
                    .ToList()
            };

            store.InstalledRuntimes.Add(new StoreRuntimeDoc
            {
                Id = "embedded_whisper_rs",
                DisplayName = "Embedded whisper-rs",
                Kind = "whisper_rs",
                EntryPath = _modelsDir,
                DiskUsageBytes = ComputeDirectorySize(_modelsDir),
            });

            if (Directory.Exists(serverRuntimeRoot))
            {
                store.InstalledRuntimes.Add(new StoreRuntimeDoc
                {
                    Id = "server_python_asr",
                    DisplayName = "Server Python ASR",
                    Kind = "faster_whisper",
                    EntryPath = serverRuntimeRoot,
                    DiskUsageBytes = ComputeDirectorySize(serverRuntimeRoot),
                });
            }

            foreach (var source in _catalogSource)
            {
                if (string.Equals(source.RuntimeId, "cloud", StringComparison.OrdinalIgnoreCase))
                {
                    continue;
                }

                if (source.CanDownload)
                {
                    var localPath = Path.Combine(_modelsDir, source.FileName);
                    if (!File.Exists(localPath))
                    {
                        continue;
                    }
                    var localPathNorm = NormalizePath(localPath);
                    var isDefault = !string.Equals(backend, "server", StringComparison.OrdinalIgnoreCase)
                        && localPathNorm == activePath;
                    if (isDefault)
                    {
                        store.ActiveModelId = source.Id;
                    }
                    store.InstalledModels.Add(new StoreModelDoc
                    {
                        Id = source.Id,
                        RuntimeId = "embedded_whisper_rs",
                        DirectoryPath = localPath,
                        SizeBytes = new FileInfo(localPath).Length,
                        IsDefault = isDefault,
                        Health = "ok",
                        RequiredFiles = new List<string> { source.FileName }
                    });
                    continue;
                }

                if (string.Equals(source.RuntimeId, "server", StringComparison.OrdinalIgnoreCase))
                {
                    var runtimePath = GetRuntimeProfileModelPath(source);
                    if (string.IsNullOrWhiteSpace(runtimePath) || (!Directory.Exists(runtimePath) && !File.Exists(runtimePath)))
                    {
                        continue;
                    }
                    var runtimePathNorm = NormalizePath(runtimePath);
                    var isDefault = string.Equals(backend, "server", StringComparison.OrdinalIgnoreCase)
                        && runtimePathNorm == activePath;
                    if (isDefault)
                    {
                        store.ActiveModelId = source.Id;
                    }
                    store.InstalledModels.Add(new StoreModelDoc
                    {
                        Id = source.Id,
                        RuntimeId = "server_python_asr",
                        DirectoryPath = runtimePath,
                        SizeBytes = Directory.Exists(runtimePath)
                            ? ComputeDirectorySize(runtimePath)
                            : new FileInfo(runtimePath).Length,
                        IsDefault = isDefault,
                        Health = "ok",
                    });
                }
            }

            var parentDir = Path.GetDirectoryName(_storePath);
            if (!string.IsNullOrWhiteSpace(parentDir))
            {
                Directory.CreateDirectory(parentDir);
            }

            var json = JsonSerializer.Serialize(store, new JsonSerializerOptions
            {
                WriteIndented = true,
                DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull
            });
            File.WriteAllText(_storePath, json);
            WriteCrossAppManifest(store);
        }
        catch
        {
        }
    }

    private void WriteCrossAppManifest(SharedModelStoreDoc store)
    {
        try
        {
            var path = Path.Combine(_modelsDir, "shared_runtime_manifest.v1.json");
            var doc = new
            {
                schema_version = "shared_runtime_manifest.v1",
                updated_at = DateTimeOffset.UtcNow.ToString("O"),
                updated_by = "dictator.settings_host",
                producer = new
                {
                    app_id = "dictator.windows.settings",
                    app_version = "0.3.0",
                    runtime_policy_schema = "runtime_policy.v1",
                    model_store_schema = "shared_model_store.v1",
                    hardware_profile_schema = "hardware_profile.v1",
                    corrections_schema = "dictator_corrections.v1"
                },
                active_runtime_id = store.ActiveRuntimeId,
                active_model_id = store.ActiveModelId,
                installed_runtimes_count = store.InstalledRuntimes.Count,
                installed_models_count = store.InstalledModels.Count,
                compat = new
                {
                    contora_min_schema_support = "shared_model_store.v1",
                    dictator_min_schema_support = "shared_model_store.v1"
                }
            };
            File.WriteAllText(path, JsonSerializer.Serialize(doc, new JsonSerializerOptions
            {
                WriteIndented = true
            }));
        }
        catch
        {
        }
    }

    private static long ComputeDirectorySize(string directory)
    {
        try
        {
            if (!Directory.Exists(directory)) return 0;
            return Directory.EnumerateFiles(directory, "*", SearchOption.AllDirectories).Select(f => new FileInfo(f).Length).Sum();
        }
        catch { return 0; }
    }

    private static string FormatBytes(long bytes)
    {
        double value = bytes;
        string[] units = ["B", "KB", "MB", "GB", "TB"];
        int unit = 0;
        while (value >= 1024 && unit < units.Length - 1) { value /= 1024; unit++; }
        return unit == 0 ? $"{value:0} {units[unit]}" : $"{value:0.00} {units[unit]}";
    }

    private static string? QueryWmi(string className, string property)
    {
        using var searcher = new ManagementObjectSearcher($"SELECT {property} FROM {className}");
        foreach (ManagementObject obj in searcher.Get())
        {
            var value = obj[property]?.ToString();
            if (!string.IsNullOrWhiteSpace(value)) return value;
        }
        return null;
    }

    private static int QueryWmiInt(string className, string property)
    {
        using var searcher = new ManagementObjectSearcher($"SELECT {property} FROM {className}");
        foreach (ManagementObject obj in searcher.Get())
        {
            if (int.TryParse(obj[property]?.ToString(), out var value)) return value;
        }
        return 0;
    }

    private static ulong QueryWmiULong(string className, string property)
    {
        using var searcher = new ManagementObjectSearcher($"SELECT {property} FROM {className}");
        foreach (ManagementObject obj in searcher.Get())
        {
            if (ulong.TryParse(obj[property]?.ToString(), out var value)) return value;
        }
        return 0;
    }

    private static double GetTotalRamGb()
    {
        using var searcher = new ManagementObjectSearcher("SELECT TotalPhysicalMemory FROM Win32_ComputerSystem");
        foreach (ManagementObject obj in searcher.Get())
        {
            if (double.TryParse(obj["TotalPhysicalMemory"]?.ToString(), out var bytes)) return bytes / (1024.0 * 1024.0 * 1024.0);
        }
        return 0;
    }

    private sealed class RuntimeProfileState
    {
        public string ModelId { get; set; } = string.Empty;
        public string ExecutionModelRef { get; set; } = string.Empty;
        public string LocalModelPath { get; set; } = string.Empty;
        public string PythonPath { get; set; } = string.Empty;
        public DateTime InstalledAtUtc { get; set; }
    }

    private sealed class SharedModelStoreDoc
    {
        [JsonPropertyName("schema_version")]
        public string SchemaVersion { get; set; } = "shared_model_store.v1";

        [JsonPropertyName("store_version")]
        public int StoreVersion { get; set; } = 1;

        [JsonPropertyName("models_root_path")]
        public string ModelsRootPath { get; set; } = string.Empty;

        [JsonPropertyName("active_runtime_id")]
        public string? ActiveRuntimeId { get; set; }

        [JsonPropertyName("active_model_id")]
        public string? ActiveModelId { get; set; }

        [JsonPropertyName("updated_at")]
        public string? UpdatedAt { get; set; }

        [JsonPropertyName("updated_by")]
        public string UpdatedBy { get; set; } = "dictator";

        [JsonPropertyName("installed_runtimes")]
        public List<StoreRuntimeDoc> InstalledRuntimes { get; set; } = new();

        [JsonPropertyName("installed_models")]
        public List<StoreModelDoc> InstalledModels { get; set; } = new();
    }

    private sealed class StoreRuntimeDoc
    {
        [JsonPropertyName("id")]
        public string Id { get; set; } = string.Empty;

        [JsonPropertyName("display_name")]
        public string DisplayName { get; set; } = string.Empty;

        [JsonPropertyName("kind")]
        public string Kind { get; set; } = string.Empty;

        [JsonPropertyName("version")]
        public string? Version { get; set; }

        [JsonPropertyName("entry_path")]
        public string EntryPath { get; set; } = string.Empty;

        [JsonPropertyName("disk_usage_bytes")]
        public long? DiskUsageBytes { get; set; }
    }

    private sealed class StoreModelDoc
    {
        [JsonPropertyName("id")]
        public string Id { get; set; } = string.Empty;

        [JsonPropertyName("runtime_id")]
        public string RuntimeId { get; set; } = string.Empty;

        [JsonPropertyName("directory_path")]
        public string DirectoryPath { get; set; } = string.Empty;

        [JsonPropertyName("size_bytes")]
        public long? SizeBytes { get; set; }

        [JsonPropertyName("is_default")]
        public bool? IsDefault { get; set; }

        [JsonPropertyName("health")]
        public string Health { get; set; } = "unknown";

        [JsonPropertyName("required_files")]
        public List<string>? RequiredFiles { get; set; }

        [JsonPropertyName("registered_at")]
        public string? RegisteredAt { get; set; }
    }

    private sealed class HistoryEntryItem
    {
        public string Id { get; set; } = string.Empty;
        public string Title { get; set; } = string.Empty;
        public string Preview { get; set; } = string.Empty;
        public string AudioPath { get; set; } = string.Empty;
        public string TextPath { get; set; } = string.Empty;
        public string MetaPath { get; set; } = string.Empty;
        public DateTime SortKey { get; set; }
    }

    private sealed class CorrectionEntryItem
    {
        public string From { get; set; } = string.Empty;
        public string Label { get; set; } = string.Empty;
    }

    private sealed class CorrectionsDocModel
    {
        [JsonPropertyName("schema_version")]
        public string SchemaVersion { get; set; } = "dictator_corrections.v1";

        [JsonPropertyName("updated_at")]
        public string UpdatedAt { get; set; } = string.Empty;

        [JsonPropertyName("entries")]
        public List<CorrectionsDocEntry> Entries { get; set; } = new();
    }

    private sealed class CorrectionsDocEntry
    {
        [JsonPropertyName("from")]
        public string From { get; set; } = string.Empty;

        [JsonPropertyName("to")]
        public string To { get; set; } = string.Empty;

        [JsonPropertyName("hits")]
        public ulong Hits { get; set; }
    }

    public sealed class CatalogModelItem : INotifyPropertyChanged
    {
        private bool _isInstalled;
        private bool _isActive;
        private bool _isDownloading;
        private bool _hasPartialDownload;
        private long _installedBytes;
        private long? _reportedSizeBytes;
        private double _downloadProgress;
        private double _suitabilityScore;
        private string _downloadStatusText = string.Empty;

        public required string Id { get; init; }
        public required string Name { get; init; }
        public required string RuntimeId { get; init; }
        public required string ExecutionModelRef { get; init; }
        public required string FileName { get; init; }
        public required long SizeBytes { get; init; }
        public required string Description { get; init; }
        public required int Speed10 { get; init; }
        public required int Accuracy10 { get; init; }
        public required string Url { get; init; }
        public required string[] LanguageTags { get; init; }
        public required bool CanDownload { get; init; }

        public bool IsInstalled
        {
            get => _isInstalled;
            set
            {
                if (_isInstalled == value) return;
                _isInstalled = value;
                OnPropertyChanged();
                OnPropertyChanged(nameof(StateLabel));
                OnPropertyChanged(nameof(StateBrush));
                OnPropertyChanged(nameof(CardBackground));
                OnPropertyChanged(nameof(CardBorderBrush));
                OnPropertyChanged(nameof(ExternalActionLabel));
                OnPropertyChanged(nameof(ExternalActionEnabled));
                OnPropertyChanged(nameof(DownloadVisibility));
                OnPropertyChanged(nameof(DeleteVisibility));
                OnPropertyChanged(nameof(DownloadEnabled));
                OnPropertyChanged(nameof(DeleteEnabled));
                OnPropertyChanged(nameof(ExternalActionLabel));
            }
        }

        public bool IsActive
        {
            get => _isActive;
            set
            {
                if (_isActive == value) return;
                _isActive = value;
                OnPropertyChanged();
                OnPropertyChanged(nameof(StateLabel));
                OnPropertyChanged(nameof(StateBrush));
                OnPropertyChanged(nameof(CardBackground));
                OnPropertyChanged(nameof(CardBorderBrush));
                OnPropertyChanged(nameof(ExternalActionLabel));
                OnPropertyChanged(nameof(ExternalActionEnabled));
            }
        }

        public bool IsDownloading
        {
            get => _isDownloading;
            set
            {
                if (_isDownloading == value) return;
                _isDownloading = value;
                OnPropertyChanged();
                OnPropertyChanged(nameof(DownloadVisibility));
                OnPropertyChanged(nameof(DeleteEnabled));
                OnPropertyChanged(nameof(ExternalActionLabel));
                OnPropertyChanged(nameof(DownloadEnabled));
                OnPropertyChanged(nameof(ProgressVisibility));
                OnPropertyChanged(nameof(ExternalActionEnabled));
                OnPropertyChanged(nameof(DownloadActionLabel));
            }
        }

        public bool HasPartialDownload
        {
            get => _hasPartialDownload;
            set
            {
                if (_hasPartialDownload == value) return;
                _hasPartialDownload = value;
                OnPropertyChanged();
                OnPropertyChanged(nameof(DownloadActionLabel));
            }
        }

        public long InstalledBytes
        {
            get => _installedBytes;
            set
            {
                if (_installedBytes == value) return;
                _installedBytes = value;
                OnPropertyChanged();
                OnPropertyChanged(nameof(SizeLine));
            }
        }


        public long? ReportedSizeBytes
        {
            get => _reportedSizeBytes;
            set
            {
                if (_reportedSizeBytes == value) return;
                _reportedSizeBytes = value;
                OnPropertyChanged();
                OnPropertyChanged(nameof(SizeLine));
            }
        }
        public double DownloadProgress
        {
            get => _downloadProgress;
            set
            {
                if (Math.Abs(_downloadProgress - value) < 0.001) return;
                _downloadProgress = value;
                OnPropertyChanged();
            }
        }

        public double SuitabilityScore
        {
            get => _suitabilityScore;
            set
            {
                if (Math.Abs(_suitabilityScore - value) < 0.001) return;
                _suitabilityScore = value;
                OnPropertyChanged();
                OnPropertyChanged(nameof(SuitabilityLine));
            }
        }

        public string DownloadStatusText
        {
            get => _downloadStatusText;
            set
            {
                if (string.Equals(_downloadStatusText, value, StringComparison.Ordinal)) return;
                _downloadStatusText = value;
                OnPropertyChanged();
                OnPropertyChanged(nameof(ProgressVisibility));
                OnPropertyChanged(nameof(ExternalActionEnabled));
            }
        }

        public string StatsLine => $"Speed {Speed10}/10 | Accuracy {Accuracy10}/10";
        public string LanguageLine => $"Tags: {string.Join(", ", LanguageTags)}";
        public string RuntimeLine => string.Equals(RuntimeId, "cloud", StringComparison.OrdinalIgnoreCase) ? "Processing: Cloud" : "Processing: Local";
        public string ExternalActionLabel => "Connect Cloud";
        public string DownloadActionLabel => HasPartialDownload ? "Resume" : "Download";
        public string SizeLine => string.Equals(RuntimeId, "cloud", StringComparison.OrdinalIgnoreCase) ? "Cloud" : FormatBytes(IsInstalled ? InstalledBytes : (ReportedSizeBytes ?? SizeBytes));
        public string SuitabilityLine => $"Device fit: {SuitabilityScore:0.0}/10";

        public string StateLabel => IsActive ? "Currently active" : IsInstalled ? "Installed" : RuntimeId == "cloud" ? "Cloud profile" : "Not installed";
        public Brush StateBrush => new SolidColorBrush(
            IsActive ? Colors.LightGreen :
            IsInstalled ? Colors.DodgerBlue :
            RuntimeId == "cloud" ? Colors.Goldenrod : Colors.DarkGray
        );
        public Brush CardBackground => IsActive
            ? new SolidColorBrush(Windows.UI.Color.FromArgb(64, 26, 120, 70))
            : new SolidColorBrush(Windows.UI.Color.FromArgb(26, 255, 255, 255));
        public Brush CardBorderBrush => IsActive
            ? new SolidColorBrush(Windows.UI.Color.FromArgb(200, 120, 220, 140))
            : new SolidColorBrush(Windows.UI.Color.FromArgb(90, 180, 180, 180));

        public bool IsLocalInstallable => CanDownload || string.Equals(RuntimeId, "server", StringComparison.OrdinalIgnoreCase);
        public Visibility DownloadVisibility => IsLocalInstallable && !IsInstalled && !IsDownloading ? Visibility.Visible : Visibility.Collapsed;
        public Visibility ExternalActionVisibility => string.Equals(RuntimeId, "cloud", StringComparison.OrdinalIgnoreCase) ? Visibility.Visible : Visibility.Collapsed;
        public Visibility DeleteVisibility => IsInstalled ? Visibility.Visible : Visibility.Collapsed;
        public Visibility CancelVisibility => IsDownloading ? Visibility.Visible : Visibility.Collapsed;
        public bool ExternalActionEnabled => string.Equals(RuntimeId, "cloud", StringComparison.OrdinalIgnoreCase);
        public bool DownloadEnabled => IsLocalInstallable && !IsDownloading && !IsInstalled;
        public bool DeleteEnabled => !IsDownloading && IsInstalled;
        public bool CancelEnabled => IsDownloading;
        public Visibility ProgressVisibility => IsDownloading || !string.IsNullOrWhiteSpace(DownloadStatusText) ? Visibility.Visible : Visibility.Collapsed;

        public event PropertyChangedEventHandler? PropertyChanged;

        public CatalogModelItem Clone() => new()
        {
            Id = Id,
            Name = Name,
            RuntimeId = RuntimeId,
            ExecutionModelRef = ExecutionModelRef,
            FileName = FileName,
            SizeBytes = SizeBytes,
            ReportedSizeBytes = ReportedSizeBytes,
            Description = Description,
            Speed10 = Speed10,
            Accuracy10 = Accuracy10,
            Url = Url,
            LanguageTags = LanguageTags,
            CanDownload = CanDownload,
            InstalledBytes = SizeBytes,
            IsInstalled = false,
            IsActive = false,
            IsDownloading = false,
            DownloadStatusText = string.Empty,
            DownloadProgress = 0,
            SuitabilityScore = 0,
            HasPartialDownload = false,
        };

        public void SyncFrom(CatalogModelItem source, bool installed, bool active, long sizeBytes)
        {
            IsInstalled = installed;
            IsActive = active;
            InstalledBytes = sizeBytes;

            if (!IsDownloading && string.Equals(DownloadStatusText, "Download canceled.", StringComparison.Ordinal))
            {
                DownloadStatusText = string.Empty;
            }

            if (!IsDownloading && installed)
            {
                DownloadStatusText = string.Empty;
                DownloadProgress = 0;
                HasPartialDownload = false;
            }
        }

        private void OnPropertyChanged([CallerMemberName] string? propertyName = null)
            => PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(propertyName));

        public static List<CatalogModelItem> DefaultCatalog() =>
        [
            new() { Id = "whisper-ggml-tiny", Name = "tiny", RuntimeId = "embedded", FileName = "ggml-tiny.bin", SizeBytes = 39L * 1024 * 1024, Description = "Fastest model for weak CPUs and quick dictation.", Speed10 = 10, Accuracy10 = 5, Url = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin", LanguageTags = ["multilingual", "low-latency"], CanDownload = true, ExecutionModelRef = "" },
            new() { Id = "whisper-ggml-base", Name = "base", RuntimeId = "embedded", FileName = "ggml-base.bin", SizeBytes = 74L * 1024 * 1024, Description = "Balanced quality and speed for everyday use.", Speed10 = 8, Accuracy10 = 7, Url = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin", LanguageTags = ["multilingual", "balanced"], CanDownload = true, ExecutionModelRef = "" },
            new() { Id = "whisper-ggml-small", Name = "small", RuntimeId = "embedded", FileName = "ggml-small.bin", SizeBytes = 244L * 1024 * 1024, Description = "Higher accuracy for noisy speech and mixed audio.", Speed10 = 6, Accuracy10 = 8, Url = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin", LanguageTags = ["multilingual", "quality"], CanDownload = true, ExecutionModelRef = "" },
            new() { Id = "whisper-ggml-medium", Name = "medium", RuntimeId = "embedded", FileName = "ggml-medium.bin", SizeBytes = 769L * 1024 * 1024, Description = "High-quality local transcription for powerful desktops.", Speed10 = 4, Accuracy10 = 9, Url = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.bin", LanguageTags = ["multilingual", "high-accuracy"], CanDownload = true, ExecutionModelRef = "" },
            new() { Id = "whisper-ggml-large-v3-turbo", Name = "large-v3-turbo", RuntimeId = "embedded", FileName = "ggml-large-v3-turbo.bin", SizeBytes = 874L * 1024 * 1024, Description = "Large model with better speed/quality balance.", Speed10 = 5, Accuracy10 = 9, Url = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin", LanguageTags = ["multilingual", "large", "turbo"], CanDownload = true, ExecutionModelRef = "" },
            new() { Id = "whisper-ggml-large-v3", Name = "large-v3", RuntimeId = "embedded", FileName = "ggml-large-v3.bin", SizeBytes = 2880L * 1024 * 1024, Description = "Maximum local quality for strongest machines.", Speed10 = 3, Accuracy10 = 10, Url = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3.bin", LanguageTags = ["multilingual", "max-quality"], CanDownload = true, ExecutionModelRef = "" },

            new() { Id = "nvidia-parakeet-tdt-0.6b-v3", Name = "Parakeet TDT 0.6B v3", RuntimeId = "server", FileName = "", SizeBytes = 2600L * 1024 * 1024, Description = "Very fast high-quality multilingual ASR on strong NVIDIA GPUs via server runtime.", Speed10 = 9, Accuracy10 = 9, Url = "https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3", LanguageTags = ["multilingual", "gpu", "parakeet"], CanDownload = false, ExecutionModelRef = "nvidia/parakeet-tdt-0.6b-v3" },
            new() { Id = "nvidia-canary-1b-v2", Name = "Canary 1B v2", RuntimeId = "server", FileName = "", SizeBytes = 5000L * 1024 * 1024, Description = "Strong multilingual accuracy with server runtime backend.", Speed10 = 6, Accuracy10 = 10, Url = "https://huggingface.co/nvidia", LanguageTags = ["multilingual", "gpu", "high-accuracy"], CanDownload = false, ExecutionModelRef = "nvidia/canary-1b-v2" },
            new() { Id = "ibm-granite-speech-3.3-8b", Name = "Granite Speech 3.3 8B", RuntimeId = "server", FileName = "", SizeBytes = 12000L * 1024 * 1024, Description = "High robustness in noisy audio; heavyweight model class.", Speed10 = 4, Accuracy10 = 10, Url = "https://huggingface.co/ibm-granite", LanguageTags = ["english", "gpu", "noise-robust"], CanDownload = false, ExecutionModelRef = "ibm-granite/granite-speech-3.3-8b" },
            new() { Id = "elevenlabs-scribe-v2", Name = "ElevenLabs Scribe v2", RuntimeId = "cloud", FileName = "", SizeBytes = 0, Description = "Cloud-only premium accuracy profile.", Speed10 = 8, Accuracy10 = 10, Url = "https://elevenlabs.io", LanguageTags = ["cloud", "premium"], CanDownload = false, ExecutionModelRef = "elevenlabs/scribe-v2" },
        ];
    }
}











