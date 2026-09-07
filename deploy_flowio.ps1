# Flowio 桌面端：本地构建 + 三处部署 + md5 一致性校验
#
# 用法（在 flowio 项目根目录的 PowerShell 中运行）：
#   .\deploy_flowio.ps1          # 假定已构建，仅复制 3 处 + 校验
#   .\deploy_flowio.ps1 -Build   # 先 npm run tauri build，再复制 + 校验
#
# 作用：
#   1. （可选）构建 release exe
#   2. 结束占用中的 flowio.exe（没跑不强杀，先 taskkill + sleep 3）
#   3. 复制 src-tauri\target\release\flowio.exe
#      → D:\录步\flowio.exe
#      → D:\flowio.exe
#      → %USERPROFILE%\Desktop\flowio.exe
#   4. 校验 源 + 3 处 共 4 份 md5 完全一致，不一致直接报错退出
#
# 注意：本脚本仅供你在【本机】运行（沙箱无法 tauri build）。

param([switch]$Build)

$ErrorActionPreference = "Stop"
$proj  = Split-Path -Parent $MyInvocation.MyCommand.Path
$src   = Join-Path $proj "src-tauri\target\release\flowio.exe"
$dests = @(
    "D:\录步\flowio.exe",
    "D:\flowio.exe",
    "$env:USERPROFILE\Desktop\flowio.exe"
)

if ($Build) {
    Write-Host "==> 构建中 (npm run tauri build) ..." -ForegroundColor Cyan
    Push-Location $proj
    npm run tauri build
    if ($LASTEXITCODE -ne 0) { Write-Error "构建失败，终止部署"; exit 1 }
    Pop-Location
}

if (-not (Test-Path $src)) {
    Write-Error "找不到构建产物: $src （先加 -Build 或手动构建）"
    exit 1
}

# 先结束可能占用 exe 的进程；没跑就不强杀
$proc = Get-Process flowio -ErrorAction SilentlyContinue
if ($proc) {
    Write-Host "==> 结束运行中的 flowio.exe ..." -ForegroundColor Yellow
    taskkill /F /IM flowio.exe | Out-Null
    Start-Sleep -Seconds 3
}

function md5($p) { (Get-FileHash $p -Algorithm MD5).Hash }

$srcMd5 = md5 $src
Write-Host "==> 源 md5: $srcMd5" -ForegroundColor Gray

$ok = $true
foreach ($d in $dests) {
    $dir = Split-Path $d
    if (-not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }
    Copy-Item $src $d -Force
    $m = md5 $d
    $tag = if ($m -eq $srcMd5) { "OK" } else { "MISMATCH" }
    if ($m -ne $srcMd5) { $ok = $false }
    Write-Host ("==> {0,-30} md5={1} [{2}]" -f $d, $m, $tag)
}

if ($ok) {
    Write-Host "`n部署成功：4 份（源 + 3 处）md5 一致 = $srcMd5" -ForegroundColor Green
    Write-Host "现在可以打开 Flowio，录制并验证「创建文件」场景了。" -ForegroundColor Green
} else {
    Write-Error "部署失败：md5 不一致，请检查"
    exit 1
}
