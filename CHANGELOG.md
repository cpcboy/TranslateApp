# CHANGELOG

本项目的重要变更都记在这里。格式参考 Keep a Changelog，日期用 YYYY-MM-DD。

## [Unreleased]

### D1 · 2026-10-22 — 工程骨架与窗口

**新增**

- Tauri 2 + React 19 + TypeScript + Vite 8 + Tailwind 4 工程骨架
- 主窗口：680×520、居中、无边框、透明背景、置顶、不可缩放、启动隐藏
- `⌥Space` 全局显隐（Rust 侧 `tauri-plugin-global-shortcut` 注册，失败会打日志而不是静默）
- 失焦自动隐藏，带 260ms 宽限期，避免呼出瞬间被误判
- `Esc` 隐藏窗口
- 设计 token 取自 `SENSE-v1.0/sense-proposal.html` 第 3 屏（`#08080A / #0F0F12 / #F2EFE9 / #FF5230`）
- 测试基座：Vitest（前端纯函数）+ `cargo test`（Rust 窗口逻辑）

**修复（review 后）**

- 快捷键注册失败时直接亮出窗口：原来只打日志，而 `⌥Space` 是唯一入口、又没有 Dock 图标，用户会以为应用没启动
- 失焦宽限期从「丢弃这次失焦」改为「延迟复查」：原实现会在呼出后 260ms 内切走时，把窗口永久压在其它 App 之上
- 中文输入法组字期间按 `Esc` 不再隐藏窗口（那是取消候选词）
- 呼出后重新聚焦输入框；补默认菜单，`⌘Q` 与输入框的 `⌘C/⌘V` 可用
- 窗口加入 `visibleOnAllWorkspaces`，全屏 App 下也能呼出
- CSP 由 `null` 收紧为显式白名单
- 移除未使用的 `zustand` / `zod`（D2、D3 真正用到时再加）

**变更**

- 包管理器由 pnpm 改为 npm：本机沙箱拦截 pnpm 创建 symlink（`CODEBUDDY_BROKER_DENY`），`pnpm add` 直接失败。`pnpm-lock.yaml` 未入库，改用 `package-lock.json`。

**已知限制**

- 输入框尚未接入查询，`lookup` 命令在 D3
- 全局快捷键的注册有日志证据，但物理按键需要人工确认一次（自动化脚本没有辅助功能权限）
- 全屏 App 下能否呼出依赖 `visibleOnAllWorkspaces`，需人工在全屏窗口中确认一次
- D2 开工前需要决定：模型流式文本进入卡片后，CSP 是否还要再收紧（当前 `connect-src` 已限定）

### D1 补丁 · 2026-10-22 — 快捷键避让 Gemini

**变更**

- 全局快捷键由 `⌥Space` 改为 `⌥⌘Space`。原因：Gemini 桌面端也用 `⌥Space`，两边会同时弹窗，且焦点被 Gemini 的输入框抢走——实测复现。
- 这是对方案 §2.5（`⌥Space`）的一次**有意偏离**，已记录在此。D2 设置页做出来后改为可自定义，默认值仍用 `⌥⌘Space`。
- 快捷键的按键与文案收敛到 `window.rs` 的 `hotkey()` / `HOTKEY_LABEL`，避免两处漂移。
