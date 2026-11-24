<#
.SYNOPSIS
    One-click trust installer for Nada Desktop Player public certificate.
#>

[CmdletBinding()]
param(
    [string]$CertFile = "Nada-PublicCert.cer"
)

$certPath = if (Test-Path $CertFile) { $CertFile } else { Join-Path $PSScriptRoot $CertFile }

if (-not (Test-Path $certPath)) {
    Write-Error "Certificate file '$CertFile' not found."
    exit 1
}

Write-Host "Installing Nada Public Certificate to Trusted Root..." -ForegroundColor Cyan
Import-Certificate -FilePath $certPath -CertStoreLocation "Cert:\CurrentUser\Root" | Out-Null
Write-Host "Done! Nada installers and binaries signed with this certificate are now trusted on this PC." -ForegroundColor Green
