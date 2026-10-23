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
  echo "没找到 ${APP}，构建失败了？" >&2
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

# 有稳定的签名身份就签上：TCC（屏幕录制）与钥匙串授权都绑定签名，
# ad-hoc 每次构建都会变，等于每次都要重新授权。
IDENTITY="${SENSE_SIGN_IDENTITY:-SENSE Dev}"
if security find-identity -p codesigning 2>/dev/null | grep -q "${IDENTITY}"; then
  echo "==> signing with: ${IDENTITY}"
  codesign --force --deep --sign "${IDENTITY}" "/Applications/SENSE.app" 2>&1 | sed 's/^/    /'
  codesign -dv "/Applications/SENSE.app" 2>&1 | grep -E "Authority|Signature" | sed 's/^/    /'
else
  echo "==> no signing identity \"${IDENTITY}\" found; keeping ad-hoc (re-authorize after every build)"
fi

echo "==> 启动"
open "/Applications/SENSE.app"

echo "完成。窗口应该已经出现；之后按 ⌃⌥⌘Space 可以随时呼出/收起。"
