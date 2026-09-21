param(
    [Parameter(Mandatory = $true)]
    [string]$Url,

    [Parameter(Mandatory = $true)]
    [string]$OutFile,

    [string]$Sha256 = ""
)

$ErrorActionPreference = "Stop"

function Invoke-OneDownload {
    param(
        [string]$CandidateUrl,
        [string]$Destination
    )

    for ($attempt = 1; $attempt -le 3; $attempt++) {
        if (Test-Path $Destination) {
            Remove-Item -Force $Destination -ErrorAction SilentlyContinue
        }

        Write-Host "Download attempt ${attempt}: $CandidateUrl"
        & curl.exe --fail --location --retry 2 --retry-all-errors --retry-delay 3 --connect-timeout 15 --max-time 900 --output $Destination $CandidateUrl

        if ($LASTEXITCODE -eq 0 -and (Test-Path $Destination)) {
            return $true
        }

        Start-Sleep -Seconds ([Math]::Min(5 * $attempt, 15))
    }

    return $false
}

$parent = Split-Path -Parent $OutFile
if ($parent) {
    New-Item -ItemType Directory -Force $parent | Out-Null
}

$ok = Invoke-OneDownload -CandidateUrl $Url -Destination $OutFile

if (-not $ok -and $Url.StartsWith("https://github.com/", [System.StringComparison]::OrdinalIgnoreCase)) {
    $fallback = "https://gh.catmak.name/$Url"
    Write-Warning "Primary GitHub download failed; switching to fallback: $fallback"
    $ok = Invoke-OneDownload -CandidateUrl $fallback -Destination $OutFile
}

if (-not $ok) {
    throw "Failed to download $Url from both primary and fallback sources."
}

if (-not [string]::IsNullOrWhiteSpace($Sha256)) {
    $actual = (Get-FileHash -Algorithm SHA256 -Path $OutFile).Hash.ToLowerInvariant()
    $expected = $Sha256.Trim().ToLowerInvariant()
    if ($actual -ne $expected) {
        Remove-Item -Force $OutFile -ErrorAction SilentlyContinue
        throw "SHA-256 mismatch for $Url. Expected $expected but got $actual."
    }
}

Write-Host "Downloaded successfully: $OutFile"
