# setup-emoji.ps1 - 自动配置 PowerShell Emoji 显示
$ErrorActionPreference = "Continue"

Write-Host "开始配置 Emoji 支持..." -ForegroundColor Green

# 1. 设置 UTF-8 编码
$consolePath = "HKCU:\Console"
Set-ItemProperty -Path $consolePath -Name "CodePage" -Value 65001 -ErrorAction SilentlyContinue
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$OutputEncoding = [System.Text.Encoding]::UTF8
Write-Host "✓ UTF-8 编码已设置" -ForegroundColor Green

# 2. 设置字体
$fonts = @("Cascadia Code", "Segoe UI Emoji")
$selectedFont = $null

foreach ($font in $fonts) {
    $fontPath = "HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Fonts"
    try {
        $fontValue = Get-ItemProperty -Path $fontPath -ErrorAction Stop | 
                     Select-Object -ExpandProperty $font -ErrorAction Stop
        if ($fontValue) {
            $selectedFont = $font
            break
        }
    } catch { continue }
}

if ($selectedFont) {
    Set-ItemProperty -Path $consolePath -Name "FaceName" -Value $selectedFont
    Set-ItemProperty -Path $consolePath -Name "FontSize" -Value 16384
    Write-Host "✓ 字体已设置为: $selectedFont" -ForegroundColor Green
} else {
    Write-Host "⚠ 未找到支持的字体，建议安装 Cascadia Code" -ForegroundColor Yellow
}

# 3. 添加到 PowerShell 配置文件
$profilePath = $PROFILE
$profileDir = Split-Path $profilePath -Parent
if (-not (Test-Path $profileDir)) {
    New-Item -ItemType Directory -Path $profileDir -Force | Out-Null
}

$configContent = @"

# Emoji 支持配置
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
`$OutputEncoding = [System.Text.Encoding]::UTF8
"@

if (-not (Test-Path $profilePath)) {
    New-Item -ItemType File -Path $profilePath -Force | Out-Null
}

Add-Content -Path $profilePath -Value $configContent -Encoding UTF8
Write-Host "✓ 配置已添加到 PowerShell 配置文件" -ForegroundColor Green

# 4. 测试
Write-Host ""
Write-Host "Emoji 测试:" -ForegroundColor Cyan
Write-Host "测试: 😀 🎉 🚀 ❤️ 🐛 👍"
Write-Host ""
Write-Host "重启 PowerShell 后生效" -ForegroundColor Yellow
