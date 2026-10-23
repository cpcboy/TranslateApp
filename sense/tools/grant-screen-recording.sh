#!/bin/bash
# 重新申请「屏幕录制」权限。
#
# 为什么需要它：SENSE 目前是 ad-hoc 签名，每次重新构建二进制都会变，
# macOS 的 TCC 记录绑定的是那一次构建的二进制。系统设置里的开关看起来是开的，
# 但新构建其实对不上，于是「授权了还是不行」。
# 这个脚本清掉旧记录，下次截屏时系统会为当前这份重新弹窗。
set -eu

echo "==> 退出 SENSE"
pkill -f "/Applications/SENSE.app/Contents/MacOS/sense" >/dev/null 2>&1 || true
sleep 1

echo "==> 清除旧的屏幕录制授权记录 com.sense.app"
tccutil reset ScreenCapture com.sense.app 2>&1 || true

echo "==> 重新启动"
open /Applications/SENSE.app

cat <<'TXT'

接下来（三次点击）：
  1. 在 SENSE 里按 ⌃⌥⌘X
  2. 系统弹窗问「SENSE 想要录制此电脑的屏幕」→ 点「允许」
  3. 退出 SENSE（⌘Q）再打开，然后再按 ⌃⌥⌘X

第 3 步不能省：macOS 对屏幕录制权限的变更只对新启动的进程生效。
TXT
