# Flowio Installer Code Signing Script (Test Certificate)
# Usage: Generate self-signed cert -> Import to trusted root -> Sign installer -> Verify

$ErrorActionPreference = "Stop"

# Config
$certName = "Flowio Test Certificate"
$certSubject = "CN=$certName"
$certStore = "Cert:\CurrentUser\Root"
$installerPath = "D:\projects\flowio\src-tauri\target\release\bundle\nsis"
$pfxPath = "D:\projects\flowio\scripts\flowio-test-cert.pfx"
$pfxPassword = "FlowioTest2026!"

Write-Host "=== Flowio Installer Code Signing ===" -ForegroundColor Cyan

# Find the installer exe
$exeFiles = Get-ChildItem -Path $installerPath -Filter "*-setup.exe" -ErrorAction SilentlyContinue
if ($exeFiles.Count -eq 0) {
    Write-Host "X Installer not found in: $installerPath" -ForegroundColor Red
    exit 1
}
$installerFile = $exeFiles[0].FullName
Write-Host "OK Found installer: $installerFile" -ForegroundColor Green

# Step 1: Generate self-signed certificate (if not exists)
Write-Host "`n[1/4] Generating test certificate..." -ForegroundColor Yellow
$cert = Get-ChildItem -Path "Cert:\CurrentUser\My" -CodeSigningCert | Where-Object { $_.Subject -eq $certSubject } | Select-Object -First 1

if (-not $cert) {
    Write-Host "  Creating new code signing certificate..."
    $cert = New-SelfSignedCertificate -Subject $certSubject -Type CodeSigningCert -CertStoreLocation "Cert:\CurrentUser\My" -NotAfter (Get-Date).AddYears(5)
    Write-Host "  OK Certificate created: $($cert.Thumbprint)" -ForegroundColor Green
} else {
    Write-Host "  OK Using existing certificate: $($cert.Thumbprint)" -ForegroundColor Green
}

# Step 2: Export PFX (for signtool)
Write-Host "`n[2/4] Exporting PFX file..." -ForegroundColor Yellow
if (Test-Path $pfxPath) {
    Remove-Item $pfxPath -Force
}
$cert | Export-PfxCertificate -FilePath $pfxPath -Password (ConvertTo-SecureString -String $pfxPassword -Force -AsPlainText) | Out-Null
Write-Host "  OK PFX exported: $pfxPath" -ForegroundColor Green

# Step 3: Import cert to trusted root
Write-Host "`n[3/4] Importing certificate to trusted root store..." -ForegroundColor Yellow
$existingRoot = Get-ChildItem -Path $certStore | Where-Object { $_.Thumbprint -eq $cert.Thumbprint }
if (-not $existingRoot) {
    $rootCert = Import-PfxCertificate -FilePath $pfxPath -CertStoreLocation $certStore -Password (ConvertTo-SecureString -String $pfxPassword -Force -AsPlainText)
    Write-Host "  OK Certificate imported to trusted root: $($rootCert.Thumbprint)" -ForegroundColor Green
} else {
    Write-Host "  OK Certificate already in trusted root: $($existingRoot.Thumbprint)" -ForegroundColor Green
}

# Step 4: Sign installer
Write-Host "`n[4/4] Signing installer..." -ForegroundColor Yellow
$signtool = "C:\Program Files (x86)\Windows Kits\10\bin\10.0.22621.0\x64\signtool.exe"
if (-not (Test-Path $signtool)) {
    $signtool = (Get-ChildItem "C:\Program Files (x86)\Windows Kits\10\bin\*\x64\signtool.exe" -ErrorAction SilentlyContinue | Select-Object -First 1).FullName
}
if (-not $signtool -or -not (Test-Path $signtool)) {
    Write-Host "  X signtool.exe not found. Please install Windows SDK." -ForegroundColor Red
    exit 1
}
Write-Host "  Using signtool: $signtool"

& $signtool sign /f $pfxPath /p $pfxPassword /tr http://timestamp.digicert.com /td sha256 /fd sha256 $installerFile
if ($LASTEXITCODE -ne 0) {
    Write-Host "  X Signing failed" -ForegroundColor Red
    exit 1
}
Write-Host "  OK Signing successful" -ForegroundColor Green

# Step 5: Verify signature
Write-Host "`n[Verify] Checking signature..." -ForegroundColor Yellow
& $signtool verify /pa $installerFile
if ($LASTEXITCODE -eq 0) {
    Write-Host "  OK Signature verified" -ForegroundColor Green
} else {
    Write-Host "  X Signature verification failed" -ForegroundColor Red
    exit 1
}

# Step 6: Show signature details
Write-Host "`n=== Signature Details ===" -ForegroundColor Cyan
& $signtool verify /v /pa $installerFile 2>&1 | Select-String -Pattern "(Issuer|Subject|Algorithm|Timestamp)" | ForEach-Object { Write-Host "  $_" }

Write-Host "`n=== Done ===" -ForegroundColor Green
Write-Host "Signed installer: $installerFile"
Write-Host "SmartScreen should now show '$certName' as publisher instead of 'unknown publisher'"
