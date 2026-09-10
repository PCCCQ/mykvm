<#
.SYNOPSIS
Builds the MyKVM Android receiver APK.

.DESCRIPTION
Locates the JDK and Android SDK, then runs Gradle. The Rust protocol core is
built automatically by the :app:buildRustCore task.

.PARAMETER Variant
debug (default) or release.

.PARAMETER Abis
Comma separated ABI list, e.g. "arm64-v8a". Defaults to the Gradle default.

.EXAMPLE
./scripts/build.ps1 -Variant release -Abis arm64-v8a
#>
[CmdletBinding()]
param(
    [ValidateSet("debug", "release")]
    [string]$Variant = "debug",
    [string]$Abis
)

$ErrorActionPreference = "Stop"
$androidDir = Split-Path -Parent $PSScriptRoot

# Gradle needs a JDK; JAVA_HOME wins, otherwise look in the usual places.
if (-not $env:JAVA_HOME -or -not (Test-Path "$env:JAVA_HOME\bin\java.exe")) {
    $candidates = @(
        "C:\Program Files\Eclipse Adoptium\*\bin\java.exe",
        "C:\Program Files\Java\*\bin\java.exe",
        "C:\Program Files\Microsoft\jdk*\bin\java.exe"
    )
    foreach ($pattern in $candidates) {
        $found = Get-ChildItem $pattern -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($found) { $env:JAVA_HOME = Split-Path -Parent (Split-Path -Parent $found.FullName); break }
    }
}
if (-not $env:JAVA_HOME) {
    throw "JAVA_HOME is not set and no JDK was found. Install JDK 17 and set JAVA_HOME."
}
$env:Path = "$env:JAVA_HOME\bin;$env:Path"

$gradleArgs = @(":app:assemble$(if ($Variant -eq 'release') { 'Release' } else { 'Debug' })")
if ($Abis) { $gradleArgs += "-PrustAbis=$Abis" }

Push-Location $androidDir
try {
    if (Test-Path "$androidDir\gradlew.bat") {
        & "$androidDir\gradlew.bat" @gradleArgs
    } else {
        & gradle @gradleArgs
    }
    if ($LASTEXITCODE -ne 0) { throw "Gradle failed with exit code $LASTEXITCODE" }
} finally {
    Pop-Location
}

$apk = Get-ChildItem "$androidDir\app\build\outputs\apk\$Variant" -Filter "*.apk" -ErrorAction SilentlyContinue |
    Select-Object -First 1
if ($apk) {
    Write-Host ""
    Write-Host "APK: $($apk.FullName)  ($([math]::Round($apk.Length / 1MB, 2)) MB)" -ForegroundColor Green
}