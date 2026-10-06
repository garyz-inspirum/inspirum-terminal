param(
    [Parameter(Position=0)]
    [string]$Binary,
    [switch]$Help
)

$ErrorActionPreference = 'Stop'

if ($Help -or [string]::IsNullOrWhiteSpace($Binary)) {
    @'
Usage:
  $env:WINDOWS_SIGNING_CERT_SHA1='THUMBPRINT'
  ./scripts/sign-windows-release.ps1 path\to\inspirum-terminal.exe

Signs with SHA-256 and an RFC3161 timestamp, then requires Authenticode status
Valid. This hook is not invoked by the unsigned release workflow.
'@ | Write-Host
    exit 0
}

if (-not (Test-Path -LiteralPath $Binary -PathType Leaf)) {
    throw "binary not found: $Binary"
}
if ([string]::IsNullOrWhiteSpace($env:WINDOWS_SIGNING_CERT_SHA1)) {
    throw 'WINDOWS_SIGNING_CERT_SHA1 is required'
}

$signtool = (Get-Command signtool.exe -ErrorAction Stop).Source
& $signtool sign /sha1 $env:WINDOWS_SIGNING_CERT_SHA1 /fd SHA256 /tr https://timestamp.digicert.com /td SHA256 $Binary
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}

$signature = Get-AuthenticodeSignature -LiteralPath $Binary
if ($signature.Status -ne 'Valid') {
    throw "Authenticode verification failed: $($signature.Status) $($signature.StatusMessage)"
}
Write-Host "Verified Authenticode signature: $($signature.SignerCertificate.Subject)"
