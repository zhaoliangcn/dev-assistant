@echo off
setlocal

set SCRIPT_DIR=%~dp0

"%SCRIPT_DIR%target\debug\dev-assistant.exe" --config .dev-assistant-models.toml --max-tokens 1000000 %*
