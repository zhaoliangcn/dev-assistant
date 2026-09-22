#!/usr/bin/env pwsh
# 启动 dev-assistant，并将 run.ps1 传入的所有参数透传给二进制。
# 用法示例：
#   ./run.ps1 --message "你好"
#   ./run.ps1 --web --port 9090
#   ./run.ps1 skill list

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition

& "$ScriptDir\target\debug\dev-assistant.exe" --config .dev-assistant-models.toml --max-tokens 1000000 @args
