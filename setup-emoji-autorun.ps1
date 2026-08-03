# setup-emoji-autorun.ps1
# 自动配置 PowerShell Emoji 支持并设置开机自动运行

$ErrorActionPreference = "Continue"

Write-Host "=== PowerShell Emoji 自动配置 ===" -ForegroundColor Cyan
Write-Host ""

# ==================== 第一部分：基础配置 ====================
Write-Host "[1/3] 配置 UTF-8 编码..." -ForegroundColor Yellow

$consolePath = "HKCU:\Console"

# 设置 UTF-8 代码页
Set-ItemProperty -Path $consolePath -Name "CodePage" -Value 65001 -ErrorAction SilentlyContinue

# 设置当前会话编码
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$OutputEncoding = [System.Text.Encoding]::UTF8

Write-Host "  ✓ UTF-8 编码已设置" -ForegroundColor Green

# ==================== 第二部分：字体配置 ====================
Write-Host "[2/3] 配置 Emoji 字体..." -ForegroundColor Yellow

$fonts = @("Cascadia Code", "Segoe UI Emoji", "Noto Color Emoji")
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
    Set-ItemProperty -Path $consolePath -Name "FontSize" -Value 16384  # 10pt
    Set-ItemProperty -Path $consolePath -Name "FontWeight" -Value 400  # 常规
    Write-Host "  ✓ 字体已设置为: $selectedFont" -ForegroundColor Green
} else {
    Write-Host "  ⚠ 未找到支持的字体，建议安装 Cascadia Code" -ForegroundColor Yellow
    Write-Host "  下载地址: https://github.com/microsoft/cascadia-code"
}

# ==================== 第三部分：永久配置 ====================
Write-Host "[3/3] 配置永久生效..." -ForegroundColor Yellow

# 添加到 PowerShell 配置文件
$profilePath = $PROFILE
$profileDir = Split-Path $profilePath -Parent
if (-not (Test-Path $profileDir)) {
    New-Item -ItemType Directory -Path $profileDir -Force | Out-Null
}

$configContent = @"

# === Emoji 支持配置（自动添加） ===
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
`$OutputEncoding = [System.Text.Encoding]::UTF8
"@

if (-not (Test-Path $profilePath)) {
    New-Item -ItemType File -Path $profilePath -Force | Out-Null
}

# 检查是否已存在配置，避免重复添加
if (-not (Get-Content $profilePath -Raw) -match "Emoji 支持配置") {
    Add-Content -Path $profilePath -Value $configContent -Encoding UTF8
    Write-Host "  ✓ 配置已添加到 PowerShell 配置文件" -ForegroundColor Green
} else {
    Write-Host "  ✓ 配置已存在，跳过添加" -ForegroundColor Cyan
}

# ==================== 第四部分：设置自动运行 ====================
Write-Host ""
Write-Host "=== 设置自动运行 ===" -ForegroundColor Cyan

# 方式一：通过任务计划程序设置开机运行
Write-Host "正在创建开机自动运行任务..." -ForegroundColor Yellow

$taskName = "PowerShellEmojiConfig"
$scriptPath = $MyInvocation.MyCommand.Path
$scriptDir = Split-Path $scriptPath -Parent

# 构建任务操作
$action = New-ScheduledTaskAction -Execute "PowerShell.exe" -Argument "-ExecutionPolicy Bypass -WindowStyle Hidden -File `"$scriptPath`""

# 构建触发器（开机时运行）
$trigger = New-ScheduledTaskTrigger -AtLogOn

# 构建设置
$settings = New-ScheduledTaskSettingsSet -StartWhenAvailable -DontStopOnIdleEnd

# 注册任务
try {
    Unregister-ScheduledTask -TaskName $taskName -Confirm:$false -ErrorAction SilentlyContinue
    Register-ScheduledTask -TaskName $taskName -Action $action -Trigger $trigger -Settings $settings -Description "自动配置 PowerShell Emoji 显示" -Force | Out-Null
    Write-Host "  ✓ 已创建开机自动运行任务: $taskName" -ForegroundColor Green
} catch {
    Write-Host "  ⚠ 任务计划程序配置失败: $_" -ForegroundColor Yellow
    Write-Host "  请手动以管理员身份运行脚本"
}

# 方式二：添加到启动文件夹（备用方案）
$startupPath = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\Startup"
$shortcutPath = Join-Path $startupPath "EmojiConfig.lnk"

if (-not (Test-Path $shortcutPath)) {
    try {
        $WshShell = New-Object -ComObject WScript.Shell
        $Shortcut = $WshShell.CreateShortcut($shortcutPath)
        $Shortcut.TargetPath = "PowerShell.exe"
        $Shortcut.Arguments = "-ExecutionPolicy Bypass -WindowStyle Hidden -File `"$scriptPath`""
        $Shortcut.WorkingDirectory = $scriptDir
        $Shortcut.Description = "Emoji 配置"
        $Shortcut.Save()
        Write-Host "  ✓ 已添加到启动文件夹" -ForegroundColor Green
    } catch {
        Write-Host "  ⚠ 启动文件夹配置失败: $_" -ForegroundColor Yellow
    }
} else {
    Write-Host "  ✓ 启动快捷方式已存在" -ForegroundColor Cyan
}

# ==================== 第五部分：测试验证 ====================
Write-Host ""
Write-Host "=== 配置完成 ===" -ForegroundColor Green
Write-Host ""
Write-Host "Emoji 测试:" -ForegroundColor Cyan
Write-Host "测试: 😀 🎉 🚀 ❤️ 🐛 👍 🌈"
Write-Host ""
Write-Host "重启 PowerShell 后生效" -ForegroundColor Yellow
Write-Host "下次开机将自动运行配置" -ForegroundColor Yellow
Write-Host ""
Write-Host "如需卸载自动运行，执行:" -ForegroundColor Gray
Write-Host "  Unregister-ScheduledTask -TaskName PowerShellEmojiConfig -Confirm:`$false" -ForegroundColor Gray
