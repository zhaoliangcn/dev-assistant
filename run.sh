#!/usr/bin/env bash
# 启动 dev-assistant，并将 run.sh 传入的所有参数透传给二进制。
# 用法示例：
#   ./run.sh --message "你好"
#   ./run.sh --web --port 9090
#   ./run.sh skill list
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

exec "$SCRIPT_DIR/target/debug/dev-assistant" --config .dev-assistant-models.toml "$@"