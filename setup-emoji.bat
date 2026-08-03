@echo off
REM 自动配置 PowerShell Emoji 显示
REM 双击此批处理文件即可运行

@echo off
echo ========================================
echo   PowerShell Emoji 自动配置工具
echo ========================================
echo.

REM 检查是否以管理员权限运行
net session >nul 2>&1
if %errorLevel% neq 0 (
    echo [警告] 建议以管理员身份运行以获得最佳效果
    echo.
)

REM 获取脚本路径
set "SCRIPT_PATH=%~dp0setup-emoji-autorun.ps1"

REM 检查脚本是否存在
if not exist "%SCRIPT_PATH%" (
    echo [错误] 找不到配置文件: %SCRIPT_PATH%
    echo 请确保 setup-emoji-autorun.ps1 与此文件在同一目录
    pause
    exit /b 1
)

REM 运行 PowerShell 脚本
echo 正在配置 Emoji 支持...
powershell -ExecutionPolicy Bypass -File "%SCRIPT_PATH%"

echo.
echo ========================================
echo 配置完成！
echo 重启 PowerShell 后生效
echo ========================================
pause
