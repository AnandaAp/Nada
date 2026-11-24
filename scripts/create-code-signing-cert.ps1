<#
.SYNOPSIS
    Generates a Code Signing Certificate for Nada Desktop Player,
    exports PFX/Base64 for GitHub Secrets, and installs it locally to Trusted Roots.

.DESCRIPTION
    Run this script on Windows (PowerShell as Administrator recommended to trust root).
#>

[CmdletBinding()]
param(
    [string]$PublisherName = "Ananda",
    [string]$Password = "Nada123456!",
    [switch]$TrustLocally = $false
)

$ErrorActionPreference = "Stop"

Write-Host "==========================================================" -ForegroundColor Cyan
Write-Host "       NADA DESKTOP PLAYER - CODE SIGNING SETUP           " -ForegroundColor Cyan
Write-Host "==========================================================" -ForegroundColor Cyan

$outputDir = Join-Path $PSScriptRoot "..\certificates"
if (-not (Test-Path $outputDir)) {
    New-Item -ItemType Directory -Path $outputDir -Force | Out-Null
}

$certSubject = "CN=$PublisherName, O=Nada Desktop Player"
Write-Host "`n[1/4] Generating Code Signing Certificate for: $certSubject..." -ForegroundColor Yellow

$cert = New-SelfSignedCertificate `
    -Type CodeSigningCert `
    -Subject $certSubject `
    -KeySpec Signature `
    -KeyUsage DigitalSignature `
    -KeyAlgorithm RSA `
    -KeyLength 2048 `
    -HashAlgorithm SHA256 `
    -CertStoreLocation "Cert:\CurrentUser\My" `
    -NotAfter (Get-Date).AddYears(5)

Write-Host "  -> Thumbprint: $($cert.Thumbprint)" -ForegroundColor Green

# 2. Export PFX
$pfxPath = Join-Path $outputDir "nada-codesigning.pfx"
$secPassword = ConvertTo-SecureString -String $Password -Force -AsPlainText
Write-Host "`n[2/4] Exporting PFX certificate to: $pfxPath..." -ForegroundColor Yellow
Export-PfxCertificate -Cert $cert -FilePath $pfxPath -Password $secPassword | Out-Null
Write-Host "  -> PFX saved successfully." -ForegroundColor Green

# 3. Export Public CER & Base64 for GitHub Actions
$cerPath = Join-Path $outputDir "Nada-PublicCert.cer"
Write-Host "`n[3/4] Exporting Public Certificate and Base64..." -ForegroundColor Yellow
Export-Certificate -Cert $cert -FilePath $cerPath | Out-Null

$base64Cert = [System.Convert]::ToBase64String([System.IO.File]::ReadAllBytes($pfxPath))
$base64Path = Join-Path $outputDir "github-secret-base64.txt"
$base64Cert | Out-File -FilePath $base64Path -Encoding ascii
Write-Host "  -> Public CER saved: $cerPath" -ForegroundColor Green
Write-Host "  -> Base64 string saved: $base64Path" -ForegroundColor Green

# 4. Local Trust
if ($TrustLocally) {
    Write-Host "`n[4/4] Installing Public Certificate to Trusted Root..." -ForegroundColor Yellow
    certutil -user -addstore Root $cerPath | Out-Null
    Write-Host "  -> Successfully trusted on this PC!" -ForegroundColor Green
} else {
    Write-Host "`n[4/4] To trust this certificate on this PC (removes SmartScreen warning):" -ForegroundColor Yellow
    Write-Host "      Run: certutil -user -addstore Root `"$cerPath`"" -ForegroundColor Cyan
}

Write-Host "`n==========================================================" -ForegroundColor Cyan
Write-Host "                     SETUP COMPLETED!                     " -ForegroundColor Cyan
Write-Host "==========================================================" -ForegroundColor Cyan
Write-Host "`nTo enable automated Code Signing on GitHub Actions:"
Write-Host "1. Go to: GitHub Repo -> Settings -> Secrets and variables -> Actions"
Write-Host "2. Add secret 'WINDOWS_CERTIFICATE':"
Write-Host "   Value: Content of '$base64Path'"
Write-Host "3. Add secret 'WINDOWS_CERTIFICATE_PASSWORD':"
Write-Host "   Value: '$Password'"
Write-Host "==========================================================`n"
