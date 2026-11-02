# Native Windows speech-to-text via System.Speech (SAPI).
# Usage: powershell -NoProfile -ExecutionPolicy Bypass -File windows_transcribe.ps1 <wav-path>
param(
    [Parameter(Mandatory = $true)]
    [string]$AudioPath
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path -LiteralPath $AudioPath)) {
    Write-Error "File not found: $AudioPath"
    exit 2
}

Add-Type -AssemblyName System.Speech

$culture = [System.Globalization.CultureInfo]::GetCultureInfo("en-US")
try {
    $engine = New-Object System.Speech.Recognition.SpeechRecognitionEngine($culture)
} catch {
    $engine = New-Object System.Speech.Recognition.SpeechRecognitionEngine
}

try {
    $engine.LoadGrammar((New-Object System.Speech.Recognition.DictationGrammar))
    $engine.SetInputToWaveFile($AudioPath)
    $result = $engine.Recognize()

    if ($null -eq $result -or [string]::IsNullOrWhiteSpace($result.Text)) {
        Write-Error "No speech recognized"
        exit 5
    }

    $segments = @()
    if ($null -ne $result.Words -and $result.Words.Count -gt 0) {
        # Group words into ~phrase-sized chunks (~12 words)
        $buf = New-Object System.Collections.Generic.List[object]
        $idx = 0
        foreach ($w in $result.Words) {
            $buf.Add($w) | Out-Null
            $idx++
            if ($buf.Count -ge 12 -or $idx -eq $result.Words.Count) {
                $start = [double]$buf[0].AudioPosition.TotalSeconds
                $last = $buf[$buf.Count - 1]
                $end = [double]($last.AudioPosition.TotalSeconds + $last.AudioDuration.TotalSeconds)
                if ($end -le $start) { $end = $start + 0.4 }
                $text = ($buf | ForEach-Object { $_.Text }) -join " "
                $segments += [pscustomobject]@{
                    text  = $text
                    start = $start
                    end   = $end
                }
                $buf.Clear()
            }
        }
    }

    if ($segments.Count -eq 0) {
        $segments = @(
            [pscustomobject]@{
                text  = $result.Text.Trim()
                start = 0.0
                end   = [Math]::Max(1.0, [double]$result.AudioDuration.TotalSeconds)
            }
        )
    }

    $payload = [pscustomobject]@{
        text     = $result.Text.Trim()
        segments = $segments
    }

    $payload | ConvertTo-Json -Compress -Depth 4
    exit 0
}
finally {
    if ($null -ne $engine) {
        $engine.Dispose()
    }
}
