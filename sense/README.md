# SENSE

macOS 上的 AI 翻译器。查一个词，给一张完整的义项卡——音标、按常用度排序的义项、场景化例句；长文给逐处批注；截图给即时解析。不做聊天框。

**当前进度：D1 / 15**。工程骨架已跑通，还没接模型。

## 现在能做什么

- `⌥Space` 在任意 App 下调出 / 收起窗口（Rust 侧注册的全局快捷键）
- 无边框、透明、置顶、居中的 680×520 窗口，启动时隐藏
- 失焦自动隐藏（260ms 宽限期，避免「刚呼出就自己藏起来」）
- `Esc` 隐藏

输入框还不能查询——`lookup` 命令、Keychain、缓存分别是 D2–D5 的事。

## 开发

环境：Node 20.19+ / 22.12+、Rust 1.85+（`edition = "2024"`）、Xcode Command Line Tools（不需要完整 Xcode）。

```bash
cd sense
npm install
npm run tauri dev
```

### 测试

```bash
npm test                     # Vitest，纯函数单测
npm run typecheck            # tsc --noEmit
cd src-tauri && cargo test   # Rust 单测（窗口宽限期逻辑）
```

### 构建

```bash
npm run build          # 只构建前端（tsc + vite build），已验证
npm run tauri build    # 出 .app / DMG；D14 打包时才正式用
```

### 本机验证用环境变量（仅开发）

| 变量 | 作用 |
|---|---|
| `SENSE_SHOW_ON_START=1` | 启动即显示窗口。默认是隐藏的，只能靠 `⌥Space` 呼出 |
| `SENSE_NO_AUTOHIDE=1` | 关闭失焦隐藏，便于截图和录屏 |

```bash
SENSE_SHOW_ON_START=1 SENSE_NO_AUTOHIDE=1 npm run tauri dev
```

## 目录

```
sense/
├─ src-tauri/
│  ├─ src/lib.rs          应用装配：菜单、插件、快捷键、启动行为
│  ├─ src/window.rs       窗口显隐、失焦宽限期（含单测）
│  ├─ capabilities/       前端权限（只开 core:default）
│  └─ tauri.conf.json     窗口、CSP 与打包配置
├─ src/
│  ├─ App.tsx             D1 空窗口外壳
│  ├─ lib/cn.ts           className 拼接（D4 复用）
│  └─ styles/index.css    Tailwind 4 + 设计 token
└─ ../SENSE-v1.0/spec/    契约与 prompt（随仓库版本化，D3 拷进 resources/）
```

## 约定

改任何代码前先读 `../MAINTENANCE_RULES.md` 与 `../AGENTS.md`，按六步走。
六条硬约束（Key 不进 WebView、模型请求只由 Rust 发起等）见 `../AGENTS.md`。
