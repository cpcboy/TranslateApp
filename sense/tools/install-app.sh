#!/bin/bash
# 把 SENSE 装到 /Applications，并重启它。
#
# 用途：本机自测。不签名、不公证，所以只能装给自己用。
set -euo pipefail

cd "$(dirname "$0")/.."

echo "==> 构建 release 包"
npm run tauri build

APP="src-tauri/target/release/bundle/macos/SENSE.app"
if [ ! -d "$APP" ]; then
  echo "没找到 $APP，构建失败了？" >&2
  exit 1
fi

echo "==> 关闭正在运行的 SENSE"
# 注意：可执行文件叫 sense（Tauri 用 Cargo 包名），不是 SENSE。
# 按进程名杀会失败，导致旧实例占着快捷键、open 只是把旧窗口提到前面。
osascript -e 'tell application id "com.sense.app" to quit' >/dev/null 2>&1 || true
sleep 1
pkill -f "/Applications/SENSE.app/Contents/MacOS/sense" >/dev/null 2>&1 || true
pkill -f "target/release/bundle/macos/SENSE.app/Contents/MacOS/sense" >/dev/null 2>&1 || true
sleep 1

echo "==> 安装到 /Applications"
rm -rf "/Applications/SENSE.app"
cp -R "$APP" "/Applications/SENSE.app"

echo "==> 启动"
open "/Applications/SENSE.app"

echo "完成。窗口应该已经出现；之后按 ⌃⌥⌘Space 可以随时呼出/收起。"
