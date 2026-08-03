# 卸载 Emoji 自动配置脚本
# 运行此脚本可移除之前设置的自动配置

$ErrorActionPreference = "Continue"

Write-Host "=== 卸载 Emoji 自动配置 ===" -ForegroundColor Cyan
Write-Host ""

# 1. 移除任务计划程序中的任务
Write-Host "[1/3] 移除开机自动运行任务..." -ForegroundColor Yellow
try {
    Unregister-ScheduledTask -TaskName "PowerShellEmojiConfig" -Confirm:$false -ErrorAction SilentlyContinue
    Write-Host "  ✓ 任务计划程序任务已移除" -ForegroundColor Green
} catch {
    Write-Host "  - 任务不存在或已移除" -ForegroundColor Gray
}

# 2. 移除启动文件夹快捷方式
Write-Host "[2/3] 移除启动文件夹快捷方式..." -ForegroundColor Yellow
$shortcutPath = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\Startup\EmojiConfig.lnk"
if (Test-Path $shortcutPath) {
    Remove-Item -Path $shortcutPath -Force -ErrorAction SilentlyContinue
    Write-Host "  ✓ 启动快捷方式已移除" -ForegroundColor Green
} else {
    Write-Host "  - 启动快捷方式不存在" -ForegroundColor Gray
}

# 3. 清理 PowerShell 配置文件
Write-Host "[3/3] 清理 PowerShell 配置文件..." -ForegroundColor Yellow
$profilePath = $PROFILE
if (Test-Path $profilePath) {
    $content = Get-Content $profilePath -Raw
    if ($content -match "Emoji 支持配置") {
        $newContent = $content -replace "# === Emoji 支持配置\(自动添加\) ===.*?(?=\n\n|\z)", ""
        Set-Content -Path $profilePath -Value $newContent -Encoding UTF8
        Write-Host "  ✓ 已从 PowerShell 配置文件中移除 Emoji 配置" -ForegroundColor Green
    } else {
        Write-Host "  - 配置文件中未找到 Emoji 配置" -ForegroundColor Gray
    }
} else {
    Write-Host "  - PowerShell 配置文件不存在" -ForegroundColor Gray
}

Write-Host ""
Write-Host "=== 卸载完成 ===" -ForegroundColor Green
Write-Host "Emoji 自动配置已移除" -ForegroundColor Yellow
Write-Host "如需恢复，请重新运行 setup-emoji-autorun.ps1" -ForegroundColor Gray
