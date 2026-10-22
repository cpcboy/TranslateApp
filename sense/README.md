# SENSE

macOS 上的 AI 翻译器。查一个词，给一张完整的义项卡——音标、按常用度排序的义项、场景化例句；长文给逐处批注；截图给即时解析。不做聊天框。

**当前进度：M1 已完成（D1–D5）**。查词闭环可用，截图与校对还没做。

## 现在能做什么

- `⌃⌥⌘Space` 在任意 App 下调出 / 收起窗口（Rust 侧注册的全局快捷键）。键位可改，见下方「改快捷键」
- 无边框、透明、置顶、居中的 680×520 窗口，启动时隐藏
- 失焦自动隐藏（260ms 宽限期，避免「刚呼出就自己藏起来」）
- `Esc` 隐藏
- 查词：Enter 触发，模型流式返回，卡片边到边渲染（首个完整义项就画，不等整包）
- 四个 provider 预设（DeepSeek / 通义千问 / OpenRouter / Ollama）+ 自定义 base_url
- Key 只进钥匙串，设置页只回显 `sk-****abcd`
- 查过的词落本地 SQLite，第二次查询 0 网络请求

## 开发

环境：Node 20.19+ / 22.12+、Rust 1.85+（`edition = "2024"`）、Xcode Command Line Tools（不需要完整 Xcode）。

```bash
cd sense
npm install
npm run tauri dev
```

### 测试

```bash
npm test                     # Vitest：zod 契约、增量解析、与 spec JSON Schema 对拍（31 个）
npm run typecheck            # tsc --noEmit
cd src-tauri && cargo test   # Rust：SSE、缓存、快捷键、密钥掩码（39 个）
```

### 没有 Key 也能端到端跑

```bash
node tools/mock-provider.mjs          # 本地 OpenAI 兼容假服务，监听 11434
npm run tauri dev                     # 设置里选 Ollama 即可用
```

它说的是真协议（`/v1/models` + SSE），应用走的是同一条代码路径，只是数据固定。
`BAD=1` 可以模拟残缺输出，用来验降级。

### 构建

```bash
npm run build          # 只构建前端（tsc + vite build），已验证
npm run tauri build    # 出 .app / DMG；D14 打包时才正式用
```

### 本机验证用环境变量（仅开发）

| 变量 | 作用 |
|---|---|
| `SENSE_SHOW_ON_START=1` | 启动即显示窗口。默认是隐藏的，只能靠快捷键呼出 |
| `SENSE_NO_AUTOHIDE=1` | 关闭失焦隐藏，便于截图和录屏 |
| `SENSE_DEV_LOOKUP=charge` | 启动后自动查一次，用于自动化截图验证 |

```bash
SENSE_SHOW_ON_START=1 SENSE_NO_AUTOHIDE=1 npm run tauri dev
```

### 改快捷键

快捷键存在配置文件里，改完重启应用即可生效，不用重新编译：

```
~/Library/Application Support/com.sense.app/settings.json
```

```json
{ "hotkey": "ctrl+alt+cmd+space" }
```

- 修饰键：`ctrl` / `alt`（=option）/ `shift` / `cmd`（=command），用 `+` 连接，顺序随意
- 主键：`space`、单个字母、单个数字
- **必须至少含一个 `ctrl` 或 `cmd`**：只有 shift/alt 的组合（例如 `shift+space`）会抢走所有输入框的按键，会被拒绝
- 写错（缺修饰键、多个主键、JSON 坏掉、字段缺失）会回退到默认 `⌃⌥⌘Space`，终端里打印具体原因，启动日志里会写明这次键位来自哪个文件

> 目录名由 `tauri.conf.json` 的 `identifier` 决定（当前 `com.sense.app`）。改了 identifier 目录就会变，以启动日志里的「来源：…」为准。

> 为什么是 `⌃⌥⌘Space`：`⌥Space` 被 Gemini / ChatGPT 占用；`⌥⌘Space` 既是 macOS 系统符号快捷键（Finder 搜索）又被 Claude 桌面端占用。带 ⌃ 的三修饰键组合不在系统符号快捷键表里。

## 目录

```
sense/
├─ src-tauri/
│  ├─ src/lib.rs          应用装配：菜单、插件、快捷键、启动行为
│  ├─ src/window.rs       窗口显隐、失焦宽限期（含单测）
│  ├─ capabilities/       前端权限（只开 core:default）
│  └─ tauri.conf.json     窗口、CSP 与打包配置
├─ src/
│  ├─ App.tsx             外壳：查词 / 设置两个视图
│  ├─ features/lookup/    义项卡、骨架屏、错误态
│  ├─ features/settings/  Provider 与 Key 管理
│  ├─ lib/schema.ts       zod 契约镜像 + 每义项救援
│  ├─ lib/stream.ts       增量 JSON 修复（栈式闭合）
│  ├─ lib/store.ts        查询状态机（序号隔离、重试、缓存旁路）
│  └─ styles/index.css    Tailwind 4 + 设计 token
└─ ../SENSE-v1.0/spec/    契约与 prompt（随仓库版本化，D3 拷进 resources/）
```

## 约定

改任何代码前先读 `../MAINTENANCE_RULES.md` 与 `../AGENTS.md`，按六步走。
六条硬约束（Key 不进 WebView、模型请求只由 Rust 发起等）见 `../AGENTS.md`。
