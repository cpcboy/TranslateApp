# CHANGELOG

本项目的重要变更都记在这里。格式参考 Keep a Changelog，日期用 YYYY-MM-DD。

## [Unreleased]

### D1 · 2026-10-22 — 工程骨架与窗口

**新增**

- Tauri 2 + React 19 + TypeScript + Vite 8 + Tailwind 4 工程骨架
- 主窗口：680×520、居中、无边框、透明背景、置顶、不可缩放、启动隐藏
- `⌥Space` 全局显隐（Rust 侧 `tauri-plugin-global-shortcut` 注册，失败会打日志而不是静默）——同日因与 Gemini 冲突改为 `⌥⌘Space`，见下方「D1 补丁」
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

### M1 补丁 2 · 2026-10-23 — 可读性重做（UI/VI）

**变更**

- 字号整体上调：词头 28→34px、义项 13.5→16.5px、例句 11.5→14px、译文 11→13px；
  全应用最小字号从 9.5px 提到 11px（原来大量 9.5–10px 的标签在 Retina 上糊成一片）
- 字重加粗：词头与义项用 600，按钮 semibold；**去掉 `-webkit-font-smoothing: antialiased`**——
  它在深色底上会把笔画再削细一档，正是「字太细」的直接原因
- 对比度按 WCAG 重算：正文 16.9:1、次级 8.5:1、注释 5.7:1、强调 6.6:1，全部 ≥AA，多数 ≥AAA
- 面板底色 `#0F0F12 → #121217`，正文 `#F2EFE9 → #F6F3ED`，强调色提亮到 `#FF6B4A`（小字也看得清）
- 窗口 680×520 → 720×660：三个义项 + 折叠入口 + 状态栏不用滚动就能看全
- 间距放大：卡片内边距、行距、分区间隔同步调整

**修复**

- WKWebView 里 `<input placeholder>` 渲染不出中文（同样内容换成 ASCII 就能显示），
  改为自绘占位符浮层，并保留 `aria-label`

**验证**

- 主界面与设置页均截图确认；`npm test` 31 passed、`npm run typecheck`、`npm run build` 通过
- 对比度为脚本实算，不是目测

### M1 补丁 · 2026-10-23 — 装成 App 并修掉「双击没反应」

**变更**

- 窗口改为启动即显示（原来 `visible: false`）：双击图标却什么都不发生，会被当成应用坏了。全局快捷键只在应用运行时有效，这一点必须让用户看得见
- 新增 `tools/install-app.sh`：构建 → 拷进 `/Applications` → 关旧进程 → 启动
- 实测：安装后的 `/Applications/SENSE.app` 启动日志为 `hotkey registered: ⌃⌥⌘Space`，窗口获得焦点

### M1 · 2026-10-23 — 查词闭环（D2–D5）

**新增**

- Keychain 读写（service `com.sense.app`，account `provider:<id>`）；设置页只读回掩码 `sk-****abcd`
- 四个 provider 预设（DeepSeek / 通义千问 / OpenRouter / Ollama）+ 自定义 `base_url`；远端强制 https，明文 http 只允许回环地址
- Rust 侧 SSE 客户端：自写增量解码器，支持多行 data、CRLF、注释、`[DONE]`、UTF-8 跨分片
- `lookup` 命令：流式转发增量（`sense://delta`），完成后落库
- spec 两份 JSON Schema 与 prompt 落入 `src-tauri/resources/`；prompt 用 `include_str!` 内联，并有测试保证变量行被替换、示例段不丢
- zod 契约镜像：整包校验失败时逐义项救援，一个都不合法才算失败
- 义项卡 UI：音标、词频、占比条、场景标签、逐条折叠、骨架屏、错误态
- 增量 JSON 修复：栈式括号闭合（修掉了方案 §8.3 示例里「先补 ] 再补 }」的非法顺序）
- SQLite 缓存（`entries` / `clicks`，表结构照 §7.1）；命中直接返回，0 网络请求
- 校验失败自动重试一次（温度 0.2）并**跳过缓存**；失败结果标记 `stale`，不会被二次读到
- `tools/mock-provider.mjs`：本地 OpenAI 兼容假服务，用于无 Key 的端到端验证

**修复（review 后）**

- P0：缓存把失败结果写死，导致「重试」永远命中同一份坏数据。改为只缓存结构完整的输出 + 重试时 bypass + 失败即标记过期
- P1：并发查询时旧结果覆盖新结果（序号守卫写在 set 之后，等于没写）
- P1：被截断的 JSON 被 `parseFinal` 静默修复，卡片会把残缺结果当成功。现在带 `repaired` 标记并明确告知
- P1：prompt 改动不会让缓存失效。缓存 key 改为 prompt 内容哈希
- P1：明文 Key 进过 React state。改为非受控输入 + ref，只经 DOM
- P1：呼出后输入框没有焦点（D1 的回归）

**已知限制**

- 真实模型未验证：需要用户在设置页填自己的 Key（不经过对话、不落盘）
- Keychain 在未签名 dev build 上可能反复弹授权，未人工确认
- 查询中切换词不会终止上一条请求，只是前端丢弃其增量
- 截图 / 划词 / 校对（D6–D10）未开始
- DMG 打包未完成：本机 `bundle_dmg.sh` 的 grep 正则会挂（`bad regex '/dev/disk4s'`），已把 `bundle.targets` 暂设为 `["app"]`，产物 `SENSE.app` 8.86 MiB 可启动；DMG 与签名公证一并放到 D14

### D1 补丁 · 2026-10-22 — 快捷键避让 Gemini

**变更**

- 全局快捷键由 `⌥Space` 改为 `⌥⌘Space`。原因：Gemini 桌面端也用 `⌥Space`，两边会同时弹窗，且焦点被 Gemini 的输入框抢走——实测复现。
- 这是对方案 §2.5（`⌥Space`）的一次**有意偏离**，以本条为准。方案里 §1.1 第 7 条、§2.5、D1 与 D6 验收仍写 `⌥Space`（交接包只读，不改原文），照 spec 验收时以本条目为准。
- 自定义快捷键按方案排在 **D13**（快捷键面板），不是 D2；在 D13 之前默认值固定为 `⌥⌘Space`。
- 按键、日志文案与界面键帽收敛到 `window.rs` 的 `hotkey()` / `HOTKEY_LABEL` / `HOTKEY_DISPLAY`，前端通过 `hotkey_hint` 命令取值，界面不再硬编码。
- 已知待验证：`⌥⌘Space` 在 **Finder 前台**是系统「搜索」快捷键（Option-Command-空格），可能与本应用同时响应。需在 Finder 里实测一次；若撞车，改用 `⇧⌥Space`。

### D1 补丁 2 · 2026-10-23 — 快捷键改为 ⌃⌥⌘Space 并支持配置

**变更**

- 全局快捷键由 `⌥⌘Space` 改为 `⌃⌥⌘Space`。实测撞击：`⌥⌘Space` 既是 macOS 系统符号快捷键（Finder 搜索窗会一起弹出），又被 Claude 桌面端占用（会弹出 Claude 对话窗）。
- 规律：`⌥Space`（Gemini / ChatGPT）、`⌥⌘Space`（Claude / Finder）都落在 Space 上，AI 客户端默认挤在这一个键。改用带 `⌃` 的三修饰键组合，不在系统符号快捷键表内。
- 快捷键改为**配置文件驱动**：`~/Library/Application Support/com.sense.app/settings.json` 的 `hotkey` 字段，改完重启生效，无需重编译。首启会自动生成该文件。
- 解析失败（缺修饰键、多主键、不支持的主键）会打印原因并回退到默认值，不会静默失败。
- 新增 3 个单测：默认值必须是 ⌃⌥⌘Space、字母键可解析、非法输入必须报错（Rust 单测共 7 个）。

**修复（review 后）**

- 配置文件读失败（JSON 坏了、字段缺失、类型不对、无权限）原来会被静默吞掉并谎报来源为「内置默认值」，现在每种失败都带原因、带文件路径打进日志
- 键帽改为**注册成功之后**才发布：原来注册失败时界面仍显示配置的键，等于告诉用户按一个不存在的快捷键。注册失败会先自动回退到默认键，再失败才隐藏键帽并亮出窗口
- 配置写入改为「临时文件 + rename」，避免并发读到空文件
- 解析器拒绝只有 `shift`/`alt` 的组合（会抢走全局输入），补充了大小写、空格、重排和 10 组非法输入的测试
