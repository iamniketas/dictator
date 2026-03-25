param(
  [ValidateSet("cpu","cuda")]
  [string]$Torch = "cpu"
)

$ErrorActionPreference = "Stop"

Write-Host "[whisper-server] Installing Python dependencies..."
python -m pip install --upgrade pip
python -m pip install -r requirements.txt

if ($Torch -eq "cuda") {
  python -m pip install torch --index-url https://download.pytorch.org/whl/cu121
} else {
  python -m pip install torch --index-url https://download.pytorch.org/whl/cpu
}

Write-Host "[whisper-server] Done."
