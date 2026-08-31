# Flowio Store Build + Sign Pipeline
# Builds NSIS installer with Microsoft Store config, then signs with test cert

$ErrorActionPreference = "Stop"

Write-Host "=== Flowio Store Build + Sign Pipeline ===" -ForegroundColor Cyan

# Step 1: Build
Write-Host "`n[Step 1] Building NSIS installer (Store config)..." -ForegroundColor Yellow
Set-Location "D:\projects\flowio"
npm run tauri:build:store
if ($LASTEXITCODE -ne 0) {
    Write-Host "X Build failed" -ForegroundColor Red
    exit 1
}
Write-Host "OK Build completed" -ForegroundColor Green

# Step 2: Sign
Write-Host "`n[Step 2] Signing installer..." -ForegroundColor Yellow
& powershell -ExecutionPolicy Bypass -File "D:\projects\flowio\scripts\sign_installer.ps1"
if ($LASTEXITCODE -ne 0) {
    Write-Host "X Signing failed" -ForegroundColor Red
    exit 1
}

Write-Host "`n=== Pipeline Complete ===" -ForegroundColor Green
Write-Host "Signed installer is ready for Microsoft Store submission or distribution."
