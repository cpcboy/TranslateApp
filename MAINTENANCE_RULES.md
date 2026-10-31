# MAINTENANCE_RULES

> 本文件是本仓库的变更总规范，适用于**全部代码、配置、数据变更**，人与 AI 代理同等适用。
> 版本：v1.0 ｜ 建立日期：2026-10-22 ｜ 维护人：项目负责人（cpcboy）
> 强制引用位置：`AGENTS.md`（任何变更前必须先读本文件）

---

## 0. 每次任务开始前的固定动作

1. 读本文件（全文）与 `AGENTS.md`。
2. 用**一句话**复述：本次要走哪六步、涉及的风险点、验证方式。
3. 复述之后才允许改代码。**没有读文档、没有复述，不许动手。**

---

## 1. 维护铁律（原文）

> §1.1–§1.4 为用户下达的《维护铁律》**原文，逐字未改**（来源：项目负责人 2026-10-22）。
> 本文件新增的补充条款一律放 §2.1–§2.3，并标注"非原文"，不与原文混排，避免后人分不清哪句是规矩、哪句是补充。

### 1.1 六步，顺序不可变

**1. 改代码**
先复述需求和已确认的口径；先读现有代码、文档和配置，给出最小改动方案。需求有歧义，或涉及权限、流程、数据口径时，先问我，不要猜。

**2. Review**
改完先自查 diff，再让另一个 review 代理/同事做对抗 review；按 P0/P1/P2 列出问题并逐条修复。P0/P1 未清零，不许进入下一步。

**3. 写文档**
更新 README/CHANGELOG/部署文档，写清改了什么、为什么、影响面、回滚方式。文档、git、线上必须三者一致。

**4. 提交 git**
只提交本次相关文件；提交信息写清模块和目的；不提交密钥、临时文件、生产数据。

**5. 部署**
先备份/留回退点；核对线上没有被人绕过 git 改过；按项目规定的方式部署；数据库变更必须先停写、备份、迁移、校验，再启动。

**6. 真实验证**
必须用真实浏览器/真实用户路径操作验证，不能只看接口 200 或健康检查；检查权限、边界、日志无报错；把验证证据（页面/截图/状态码/日志）发给我。验证失败就回到第 1 步，不许带病上线。

### 1.2 三条底线

**A. 不直接操作生产业务数据。**
必须改时：先拿到明确授权 → 备份 → 用官方入口或可审计脚本 → 写审计日志 → 可回滚。禁止"先改了再说"。

**B. 不擅自放宽权限、流程或防呆规则。**
涉及角色、审批、锁定、拦截的变更，必须先给方案让我确认；确认后再改。

**C. 所有业务数据只存数据库/对象存储，不落本地文件。**
本地只放代码和临时验证产物，验证完清理。

### 1.3 每次交付必须给我

- 改了哪些文件、为什么；
- review 发现的问题和修复情况；
- 文档更新位置（含 MAINTENANCE_RULES.md 是否更新、更新了哪一节）；
- commit hash；
- 部署到哪、是否重启；
- 验证证据（真实浏览器/关键路径/日志）；
- 回滚方法；
- 《变更执行记录》里追加的一行。

**缺任何一项，都视为未完成。**

### 1.4 禁止事项

- 不许跳过 review 直接部署；
- 不许只部署不提交、只改线上不改仓库；
- 不许用"应该可以/理论上没问题"代替验证；
- 不许在未确认时批量改数据、批量发通知；
- 不许把密钥、密码、token 写进代码或聊天记录。

---

## 2. 本项目落地信息

> 原则：**不知道就写"待确认"，不编造。** 下表每一条都标注了状态与补齐时点。

| 项目 | 内容 | 状态 |
|---|---|---|
| **测试命令** | 在 `sense/` 下：`npm test`（Vitest 5，匹配 `src/**/*.test.{ts,tsx}`）、`npm run typecheck`（`tsc --noEmit`）、`cd src-tauri && cargo test`（Rust 单测）。2026-10-30 实测：前端 325 个用例、Rust 116 个用例（含运行辅助程序 `--selftest`），全绿 | ✅ 已验证 |
| **构建命令** | 在 `sense/` 下：`npm run build`（`tsc && vite build`，已验证）；`npm run tauri build` → `bundle/macos/SENSE.app`（**8.86 MiB，已验证可启动并注册快捷键**）。当前 `bundle.targets = ["app"]`：本机 `bundle_dmg.sh` 的 grep 正则有 bug，DMG 留到 D14 连同签名公证一起处理 | ✅ 已验证 |
| **部署命令** | **本机安装**：`cd sense && ./tools/install-app.sh`（构建 → 杀旧实例 → 拷进 `/Applications` → 启动，已验证）。脚本按可执行文件路径杀进程（`sense`，不是 `SENSE`），改名前留意。<br>**对外分发**：当前不存在。 v0.1 是本地 macOS 应用，没有服务端。D14 起分发形态才是"签名 + 公证的 DMG → GitHub Releases"，命令与证书届时确认。<br>六步里的第 5 步在 D14 之前统一记为"不涉及（无部署目标）"，D14 起记为"发布 DMG 到 Release" | D14 前：不涉及<br>D14 起：待定 |
| **线上验证地址** | **无。** 产品侧无服务器（硬约束 4：服务器成本为 0），不存在线上环境。对应的"线上验证"= 本机安装后的真实用户路径验证（`⌃⌥⌘Space` 唤起 → 查词 → 看卡片 → 断网复看缓存） | 现状，非缺失 |
| **文档路径** | 本文件 `MAINTENANCE_RULES.md`；`AGENTS.md`；`README.md`（仓库总览）；`CHANGELOG.md`（版本变更）；`sense/README.md`（工程运行说明）；交接入口 `SENSE-v1.0/START-HERE.md`；主方案 `SENSE-v1.0/docs/SENSE-落地方案.md`；契约与 prompt `SENSE-v1.0/spec/`（字段冻结，勿改） | ✅ |
| **包管理器** | **npm**（`sense/package-lock.json` 入库）。不用 pnpm：本机沙箱拦截 pnpm 创建 symlink（`CODEBUDDY_BROKER_DENY`），`pnpm add` 直接失败。若将来环境不再拦截，切回 pnpm 需要重装依赖并替换锁文件 | 现状，环境约束 |
| **回滚方式** | **代码**：`git revert <hash>`（前提：仓库已初始化，见 §2.1）。<br>**配置**：删除 `~/Library/Application Support/com.sense.app/settings.json` 即恢复默认（含快捷键），下次启动自动重建。<br>**已安装版本**：重装上一个 DMG；DMG 归档位置与命名规则 **待 D14 确定**（当前还没有任何 DMG）。<br>**缓存**：删除本地 SQLite 库文件后重启。库文件路径 **待 D5 实测确认**——Tauri identifier 尚未定，可能与 Keychain service `com.sense.app` 不同，不要按 Keychain 的 service 名去找。<br>**数据**：无服务端、无业务数据库，故不存在数据回滚 | 部分待 D5 / D14 实测 |
| **密钥与凭证** | API Key 只存 macOS Keychain（service `com.sense.app`，account `provider:<id>`）。代码、配置、日志、聊天、提交记录中均不得出现明文。设置页只回显 `sk-****abcd` | 硬约束 1，不可变更 |
| **业务数据** | 无云端业务数据。本地只有 SQLite 缓存（词条）与点击统计，可随时删除，删除后仅损失缓存 | 现状 |
| **数据库变更流程** | 目前无数据库服务。SQLite 仅在本地，schema 变更走"建新表 / 加列，不删旧列"，失败即回滚到上一版安装包 | 适用于 D5 起 |

### 2.1 分支与提交约定（非原文，本项目补充）

- 仓库：`2026-10-22` 首次 `git init`，分支 `main`。
- 远端：`origin` = `https://github.com/cpcboy/TranslateApp`（地址由项目负责人 2026-10-22 提供）。**只配置，不推送**；push 需每次单独授权。
  2026-10-27 经项目负责人单次授权，已把当时的 `main`（63 个提交）推送到 `origin`（远端原本是空仓库）；此后又经单次授权推送过三次（`fc2df79` → `537afdf` → `40ea4f3` → `f878d6c`，均为快进、未用 `--force`，GitHub 上没有 PR 与其它分支）。**之后的 push 仍需每次单独授权**，推送前先确认远端状态与将要发布的提交范围。
- 逐日提交，便于按天 diff 复盘；一个提交只做一件事，文档提交与代码提交分开。
- 提交信息前缀：`feat:` / `fix:` / `docs:` / `chore:` / `refactor:`，正文写清模块与目的。
- 禁止 `push --force`，禁止改写已推送历史。

### 2.2 本项目的额外红线（非原文，但优先级高于通用流程）

1. **Key 永不进 WebView**：不进前端 state、localStorage、日志、错误上报、崩溃栈。
2. **模型请求只由 Rust 发起**：前端不得直连 provider，不得持有 baseURL + Key 组合。
3. **契约冻结**：`SENSE-v1.0/spec/word-entry.schema.json`、`SENSE-v1.0/spec/proofread.schema.json` 字段名不可改，只可加可选字段；改契约 = 全盘返工，必须先确认。
4. **不许假装成功**：校验失败降级时必须明确告知用户，禁止把降级结果渲染成正常结果。

### 2.3 继承自交接包的既有约定（非原文，继续有效）

- 不提交真实 API Key；不 push 到远端（除非当次明确授权）。
- 一次只做一天的任务（D1–D5 节奏），做完先自验，再汇报。
- 汇报格式固定：做了什么 / 怎么验证的 / 还剩什么 / 卡在哪。
- 界面文案用中文。

---

## 3. 变更执行记录

> 填写约定：
> 1. `commit` 列写 **短 hash + 提交主题**。记录行与变更同批入库，本轮 hash 在下一次文档提交时回填；回填前先写提交主题，`git log --grep="<主题>"` 可反查。
> 2. `部署` 列：D14 之前统一写"不涉及（无部署目标）"；D14 起写"DMG → GitHub Releases"。
> 3. 行只追加，不删改；写错了用新行更正，并注明被更正的行。

| 日期 | 变更内容 | review 结论 | 文档更新 | commit | 部署 | 验证证据 | 回滚方式 |
|---|---|---|---|---|---|---|---|
| 2026-10-22 | 建立维护铁律与执行规范：新增 `MAINTENANCE_RULES.md`、`AGENTS.md`；初始化本地 git 仓库并配置远端（仅配置，未推送） | 自查 + 独立对抗 review 一轮：提出 P0 3 条 / P1 6 条；其中 2 条不成立（远端地址由负责人提供、提交尚未执行），其余 7 条已修，P0/P1 清零 | 新增两份文档；本文件 §1 原文标注来源、§2 项目信息、§3 记录表 | `5434866` | 不涉及（无部署目标） | `git show --stat` 仅这两份文件；需求 A–F 逐条比对；review 问题逐条修复 | 直接删除两份文件（首个提交，无前序状态可回滚） |
| 2026-10-22 | **D1**：工程骨架、无边框置顶窗、`⌥Space` 显隐；交接包 `spec/` 入库 | 自查 + 独立对抗 review：P0 1 条 / P1 6 条，已全部修复（热键失败兜底、宽限期改延迟复查、IME 下 Esc、呼出后重聚焦、`visibleOnAllWorkspaces`、CSP 收口、补默认菜单与退出路径）；P0/P1 清零 | 新增 `README.md`、`CHANGELOG.md`、`sense/README.md`；本文件 §2 回填测试/构建命令、文档路径、包管理器 | `56086f8`(spec) `93e2953`(feat) `ac09ea2`(fix) `3c3d03c`(docs) | 不涉及（无部署目标） | `npm test` 3 passed；`cargo test` 4 passed；`npm run build` 成功；运行日志 `hotkey registered` / `window shown (680x520)` / `window focused`；截图见交付报告 | `git revert ac09ea2 93e2953 56086f8`；或整目录删除重来 |
| 2026-10-22 | 全局快捷键 `⌥Space` → `⌥⌘Space`（Gemini 桌面端同键，双方同时弹窗且焦点被它抢走） | 自查：按键与文案收敛到单一函数、界面提示同步；`cargo test` 4 passed、`npm test` 3 passed、构建通过；无 P0/P1 | `CHANGELOG.md` 增「D1 补丁」条目（记录对方案 §2.5 的有意偏离）；`sense/README.md`、`AGENTS.md` 与界面文案同步；本文件 §2 线上验证地址改键 | `cef48c5` `6503c36` `57325e3` | 不涉及（无部署目标） | 运行日志 `hotkey registered: Alt+Command+Space (⌥⌘Space)`；截图 `/tmp/sense-hotkey.png`（680×520，实测纯色不透明） | `git revert cef48c5` |
| 2026-10-22 | 快捷键文案单一来源：前端改用 `hotkey_hint` 命令取键帽，新增 2 个快捷键单测 | 独立 review：P1 2 条——① 界面仍硬编码，与「单一来源」说法不符（已修）② `⌥⌘Space` 在 Finder 前台可能与系统「搜索」快捷键同时响应（**未验证，待人工**） | `CHANGELOG.md` 补 D13 归属、spec 偏离说明、Finder 风险；本文件 §3 | `a29d8ff` `59a4bd9` | 不涉及（无部署目标） | `cargo test` 6 passed；`npm test` 3 passed；运行截图顶部显示 `⌥⌘Space 呼出` | `git revert a29d8ff` |
| 2026-10-23 | 快捷键 `⌥⌘Space` → `⌃⌥⌘Space`；快捷键改为配置文件驱动（`settings.json` 的 `hotkey`） | 人工实测确认：`⌥⌘Space` 同时触发 Finder 搜索窗与 Claude 对话窗；系统符号快捷键表核对（`⌘Space`、`⌥⌘Space` 已占用，`⌃⌥⌘Space` 未占用）。解析逻辑补 3 个单测，无 P0/P1 | `CHANGELOG.md` 新增「D1 补丁 2」；`sense/README.md` 增加「改快捷键」说明；本文件 §2 回滚方式补配置文件 | `08edddc` `10dff5f` | 不涉及（无部署目标） | `cargo test` 7 passed；`npm test` 3 passed；日志 `hotkey registered: ⌃⌥⌘Space`；改配置为 `ctrl+alt+k` 后重启日志变为 `⌃⌥K`（证明免重编译生效） | `git revert 08edddc`；或删 `settings.json` 回默认 |
| 2026-10-23 | 修「屏幕录制授权了还是不行」：权限预检 + 错误上屏 + capture.log + 授权重置脚本 | 根因定位：ad-hoc 签名导致每次构建换 TCC 身份（`codesign -dv` 显示 adhoc / 哈希 identifier）；`cargo test` 46 passed | `CHANGELOG.md`、`sense/README.md` 权限小节、本文件 §3 | `f116b42` `f5e2f00`(chore 记录) | 本机 `/Applications/SENSE.app` | 需用户在自己终端跑 `./tools/grant-screen-recording.sh`（沙箱内 tccutil 被拒：Operation not permitted from sandbox） | 重装回上一版本 + 恢复 TCC 授权 |
| 2026-10-23 | 新增截屏翻译（Vision OCR + 原位覆盖）与自定义快捷键 | 自查 + 真机端到端：dev 矩形 520,300,760,420 走完截图→OCR→翻译→覆盖，7 块全部落位；快捷键注册日志含 ⌃⌥⌘X；`cargo test` 46 passed、`npm test` 31 passed。未做独立 review（下一轮补） | `CHANGELOG.md` 增「M2 起步」；`sense/README.md` 增「截屏翻译」「改快捷键」；本文件 §3 | `9fe07dd` `9d74181`(chore 记录) | 本机 `/Applications/SENSE.app`，需授权屏幕录制 | 截图 `/tmp/sense-capture-overlay.png`；日志 `capture hotkey registered: ⌃⌥⌘X`、`dev capture: 7 块`、`翻译请求：7 段` | `git revert` 本次提交 + 重跑 `./tools/install-app.sh` |
| 2026-10-23 | 查词提速：按 provider 关掉模型思考（DeepSeek thinking 默认开启）、max_tokens 4096、启动预热连接、增量 66ms 合流、首义项真实计时、本地模型绕开系统代理 | 独立对抗 review：P0 0 条 / P1 2 条（合流缺 epoch 守卫；400 兜底可能重复推送已发出的增量）+ P2 5 条，全部修复。P0/P1 清零 | `CHANGELOG.md` 增「M1 补丁 4」（含实测表与 500ms 结论）；`sense/README.md` 增「速度」一节；本文件 §3 | `3f6842c` `1127f60`(docs) `5696cf7`(chore 记录) | 本机 `/Applications/SENSE.app`（重装后首次查词需在钥匙串弹窗点一次「始终允许」） | mock 实测 ttft=45ms / 首义项 192ms / 总计 561ms；请求体实测 `thinking={"type":"disabled"}`、`max_tokens=4096`；`api.deepseek.com` 往返 TTFB 332ms；`cargo test` 43 passed、`npm test` 31 passed | `git revert` 本次提交 + 重跑 `./tools/install-app.sh` |
| 2026-10-23 | 窗口 960×900 可缩放（右下角把手 + header 拖动区）；字号再上一档（义项 21px、词头 46px）；灰色提亮到 11.5:1 / 8.1:1；英文例句改 Helvetica Neue 斜体；修安装脚本杀错进程名 | 独立对抗 review：P0 1 条（`startResizeDragging` 在 macOS 是静默 no-op，tao 返回 NotSupported）+ P1 1 条（`.ex-en` 把中文例句也套合成斜体）+ P2 5 条，全部修复；另自查发现安装脚本 P0（`pkill -x SENSE` 匹配不到 `sense`，装上去的还是旧版）。P0/P1 清零 | `CHANGELOG.md` 增「M1 补丁 3」；`sense/README.md` 窗口尺寸与权限说明；`tools/install-app.sh` 注释；本文件 §2 部署命令 | `9907373` `ff55205`(docs) `8788343`(chore 记录) | 本机 `/Applications/SENSE.app`，已重启（旧实例必须手动杀） | `npm test` 31 passed、`cargo test` 39 passed；CGWindowList 实测窗口 `960×900@(480,60)`、最小 `760×720` 布局截图；临时调 `setSize(820,760)` 实测生效后删除；安装后窗口截图 `/tmp/sense-installed-final.png` | `git revert` 本次提交 + 重跑 `./tools/install-app.sh` |
| 2026-10-23 | 可读性重做：字号/字重/对比度整体上调，窗口 720×660，自绘占位符 | 自查 + 真机截图（主界面与设置页）；对比度脚本实算全部 ≥AA；无 P0/P1 | `CHANGELOG.md` 增「M1 补丁 2」；`sense/README.md` 窗口尺寸 | `402cfca` `a956a7c`(docs) | 不涉及 | 截图 `/tmp/sense-fixed-win.png`、`/tmp/sense-settings-win.png`；对比度：正文 16.9:1 / 次级 8.5:1 / 注释 5.7:1；`npm test` 31 passed | `git revert` 本次提交 |
| 2026-10-23 | 装成 App：窗口改为启动即显示，新增 `tools/install-app.sh` | 自查 + 真机验证：安装后 `/Applications/SENSE.app` 启动日志 `hotkey registered: ⌃⌥⌘Space`，窗口获得焦点并可见；无 P0/P1 | `sense/README.md` 增「装成 App」、根 `README.md` 增安装段、`CHANGELOG.md` 增补丁条、本文件 §2 部署命令 | `6afda9b` | 本机 `/Applications`，无需重启 | 截图 `/tmp/sense-installed.png`；启动日志见上 | 删除 `/Applications/SENSE.app`，回到 `git revert` 上一提交 |
| 2026-10-23 | **M1（D2–D5）**：Keychain、Provider 预设、SSE、`lookup`、义项卡、增量解析、SQLite 缓存 | 独立对抗 review：P0 1 条 + P1 5 条，全部修复——① 缓存写死失败结果导致重试永远命中坏数据 ② 并发查询旧结果覆盖新结果 ③ 截断 JSON 被当成功 ④ 改 prompt 不失效缓存 ⑤ 明文 Key 进 React state ⑥ 呼出后输入框无焦点；P0/P1 清零 | 两份 README、`CHANGELOG.md` 增 M1 段、本文件 §2 测试计数、`AGENTS.md` 阶段 | `f202a8a` + `01cd88c`(docs) `8a3dc77` | 不涉及（无部署目标） | `cargo test` 39 passed；`npm test` 31 passed；`npm run typecheck`、`npm run build` 通过；mock 端到端：首次一次 POST 出卡，重启后 `cache hit` 且 mock 请求数不变（截图 /tmp/sense-reg-card.png） | `git revert f202a8a` |
| 2026-10-23 | 按 review 修复配置静默失败与键帽发布时机 | **更正上一条的「无 P0/P1」**：独立 review 指出 2 条 P1——① 配置文件读取失败被静默吞掉、来源谎报为「内置默认值」② 键帽在注册成功前就发布。均以修复；另修 4 条 P2（原子写、状态不可更新、只有 shift/alt 的组合、文档残留旧键） | `CHANGELOG.md` 增「修复（review 后）」；`sense/README.md` 补键位规则与 identifier 说明；`AGENTS.md` 与 §2 旧键文案同步；本文件 §3 | `7192437` + `d4eb0c9`(docs) | 不涉及（无部署目标） | 写坏 JSON 后启动：日志打印具体文件 + 解析错误并回退默认；`cargo test` 7 passed；`npm test` 3 passed；截图顶部为 `⌃⌥⌘Space 呼出` | `git revert 7192437` |
| 2026-10-27 | 修评审 P1 六项：长文没写完（报错中断、服务端提前收工、停止）一律标「不完整」；截屏翻译失败在覆盖层写明原因；校验失败文案按 finish_reason 说实话并带出 zod 原因；展示字段超限按 spec 截断、不再作废整卡；流式请求单独设时限（长文 879 秒，其余仍 180 秒，等响应头 180 秒）；划词浮层里 Esc 只关浮层。新增 debug 钩子 `SENSE_DEV_CAPTURE_PNG` | 自查 + 独立对抗 review：P0 0 / P1 1 / P2 8，全部修复（服务端提前收工仍显示完整、查词时限被放宽、文案分支、断流错误码、覆盖层提示被截、合流守卫、先救再截、按请求编号取消、测试缺口）；同一 reviewer 复核：无新增 P0/P1，遗留 3 条 P2 已修。P0/P1 清零 | `CHANGELOG.md`「修复 · 2026-10-27」；`sense/README.md` 截屏与长文的失败提示、测试数、环境变量；本文件 §2 测试数、§3 | `ef49ce2` `8582995` `c20e2a7` `f0d7e79` `6f85c8f` `39053ba`（fix）`b952d61`（docs）+ `fc2df79`（回填） | 不涉及（无部署目标） | 提交前：`npm test` 57 passed、`npm run typecheck` 通过、`cargo test` 57 passed；三条关键测试反向验证（去掉修复即失败）；界面实测（内置浏览器 + IPC 垫片）：round 出卡并提示「词性 5→4」、refuse/badlang 文案、长文断流显示「不完整」、浮层 Esc 只关浮层、覆盖层失败提示；真实二进制（隔离 HOME + mock）：round 1 次请求不再重试、refuse 重试 1 次后报错、截屏钩子 OCR 5 行→3 块并把失败原因交给覆盖层。提交后复验（HEAD `b952d61`，逐个提交另抽出快照单独跑过 tsc + vitest，Rust 改动的两个提交跑过 cargo test，均通过）：真实二进制 round 命中修复前写入的缓存后校验通过、不再重试，refuse 重试 1 次后报错，截屏钩子成功路径 3 块全部翻译、失败路径把原因交给覆盖层；界面复验 content_filter 提前收工、中途断开、点停止三种「不完整」提示，round、refuse、浮层 Esc、覆盖层 300×84 失败提示。未覆盖：原生窗口实操（桌面控制授权被拒）、真实 provider | `git revert` 本次各提交；无数据迁移 |
| 2026-10-23 | **补记**（2026-10-27，评审 P2-13）：当时没有写本表，这里按 git 历史与 CHANGELOG 补；review、验证凡当时没留下记录的一律写「未记录」，不追认。 截屏翻译排版与上下文：整屏当一篇文档翻、字号自适应并改按单行高度算、译文层失焦自动关闭、关掉思维链 | 未记录（当时没有独立 review 记录） | `CHANGELOG.md`「截屏翻译的版面与上下文」「截屏译文字号跳变、遮罩层关不掉」「截屏翻译压到 2 秒内」 | `4e543e1` `0aab9e1` `c848f0c`（fix / perf）`acc4ed0` `d6ad83b` `f5e2f00`（chore 记录） | 未记录 | CHANGELOG 对应条目写了现象与修法；当时的截图、命令输出没有留在本表 | `git revert` 上述提交 |
| 2026-10-23 | **补记**（2026-10-27，评审 P2-13）：当时没有写本表，这里按 git 历史与 CHANGELOG 补；review、验证凡当时没留下记录的一律写「未记录」，不追认。 构建脚本支持稳定签名身份；修安装脚本里变量名后紧跟中文导致 unbound variable、未授信证书也能签 | 未记录 | 仅 `tools/install-app.sh` 的注释；CHANGELOG 无对应条目 | `ee8ea24` `b6ab6c9`（fix / chore）`c9a5eb7`（chore 记录） | 未记录 | 未记录 | `git revert` 上述提交 |
| 2026-10-23 | **补记**（2026-10-27，评审 P2-13）：当时没有写本表，这里按 git 历史与 CHANGELOG 补；review、验证凡当时没留下记录的一律写「未记录」，不追认。 长文翻译与阅读视图：整篇流式 + 左右对照、停止生成、60ms 合流、自增高输入框与版式还原、语法波浪线与划词解析、截断提示 / ⌘⇧D / 同步滚动 / 重试与返回 | 当时未记录；2026-10-27 全量评审已补审，P1 六项见上一组记录，P2 / P3 见下一行 | `CHANGELOG.md`「长文翻译」「整篇流式 + 左右对照」「长文：停止、合流、版式还原」「语法标注与划词解析」「阅读视图收尾」 | `404c00c` `336a629` `668e94e` `8f7ffe6` `3c1735b`（feat）`0db973a`（chore 记录） | 未记录 | CHANGELOG 对应条目写了行为与已知限制；当时的验证证据没有留在本表 | `git revert` 上述提交 |
| 2026-10-23 | **补记**（2026-10-27，评审 P2-13）：当时没有写本表，这里按 git 历史与 CHANGELOG 补；review、验证凡当时没留下记录的一律写「未记录」，不追认。 截屏选区迭代：窗口按 label 路由、创建后立即取焦点，最终改用系统原生 `screencapture -i`（删掉自绘遮罩与 `finish_capture`） | 未记录 | `CHANGELOG.md`「选区改用系统原生截图」「选区窗口拖不动」「截屏选区窗口白屏 / 打不开」 | `6b5c513` `14bf916`（fix）`136e4fc`（refactor）`2d87bc6` `a8f8553`（chore 记录） | 未记录 | CHANGELOG 记录了合成鼠标拖拽 (400,400)→(900,700) 得到 `capture rect=400,400,500,300`、`ocr lines=12`；截图未留存 | `git revert` 上述提交 |
| 2026-10-23 | **补记**（2026-10-27，评审 P2-13）：当时没有写本表，这里按 git 历史与 CHANGELOG 补；review、验证凡当时没留下记录的一律写「未记录」，不追认。 查词失败可见性：格式不合格时显示模型原文，流中断时显示原因（content_filter / length / 中断）并记录输出长度与结尾 | 未记录 | 无（只有 `.workbuddy/memory` 的排查笔记，CHANGELOG 无对应条目） | `8366500` `31c7372`（fix）`7c1457c`（chore 记录） | 未记录 | 未记录 | `git revert` 上述提交 |
| 2026-10-23 | **补记**（2026-10-27，评审 P2-13）：当时没有写本表，这里按 git 历史与 CHANGELOG 补；review、验证凡当时没留下记录的一律写「未记录」，不追认。 展示类枚举兜底：`interj` 等词性、场景、语域不在名单里时不再让整张卡作废 | 未记录 | `CHANGELOG.md`「interj 之外的词性导致整卡作废」 | `b9780d5` | 未记录 | CHANGELOG 记录原用例改为「zod 兜底、spec 仍拒绝」并新增 interj 用例，当时 32 passed | `git revert b9780d5` |
| 2026-10-27 | 评审 P2 13 项 + P3 12 项全部处理：详见 `CHANGELOG.md` 同名条目。要点——关思考字段 400 兜底下沉公共层；快捷键回退键记入状态、设置页以注册状态为准；截屏落点推算（尺寸取自 PNG、以松手点为锚、对不上就明说）、后来者胜、OCR 超时、选区屏幕随选区带着走；测试连接测表单当前值、端点校验交给 URL 解析器（修掉 `?@localhost` 等绕过）；查词 / 长文分流新规则；词性语域归一与标签补全；语法标注整词 + 每问题一处；划词乱序与上下文；缓存上限与淘汰；OCR sidecar 不再入库；死代码清理；mock 与提示词防漂移；文档与记录补齐 | 自查 + 两位独立对抗 review（Rust / 前端）各复核一轮：P0 0 / P1 0；第一轮 P2 8、第二轮 P2 6，全部修复；已知未处理的 P3 写在 CHANGELOG「Review」一节 | `CHANGELOG.md`「评审 P2 / P3 全部处理」；`sense/README.md`（分流规则、截屏、测试连接、快捷键未生效、缓存上限、OCR 构建、测试数、环境变量）；本文件 §2 测试数、§3（回填 6 行「本次」，补记 6 组缺失的变更） | `9f529fa` `fe3e2dd` `1d4d667` `5c4b101` `52bae52`（fix）`f2c8bda`（chore）`44e22ee`（docs）+ 本次（回填） | 不涉及（无部署目标） | 提交前：`npm test` 107 passed、`npm run typecheck`、`npm run build` 通过、`cargo test` 90 passed、`cargo build` 无警告；新增用例均做过反向验证；真实二进制（隔离 HOME + 假网关）：400 去字段重发后记住，截屏链路只发 1 个请求，OCR → 翻译 3 块；界面（内置浏览器 + IPC 垫片）逐项实测；OCR 构建三条路径实测。未覆盖：原生窗口实操、真实 `screencapture -i` 交互、Retina / 多屏硬件、真实 provider。提交后复验（HEAD `44e22ee`）：逐个提交用 `git archive` 抽出快照单独跑 tsc + vitest + cargo test，均通过（前端 57 / 59 / 59 / 80 / 101 / 107 个，Rust 61 / 67 / 87 / 87 / 87 / 90 个；快照里没有 OCR 二进制，由 `build.rs` 现编，证明干净克隆能构建）；HEAD 上 `npm test` 107、`npm run typecheck`、`npm run build`、`cargo test` 90 通过，入库的 binaries 为 0 | `git revert` 本次各提交；无数据迁移；回滚 OCR 二进制出库后需重新提交一份或让 `build.rs` 现编 |
| 2026-10-27 | 修长文对照「停手后两栏还在互相推着滚」：同步滚动的回响（写对方 `scrollTop` 引出的 scroll 事件）被当成用户滚动又写回，WebKit 取整每个来回丢 1px，两栏一直往顶部漂。新增 `scrollSync.ts`，记下写入位置、认出回响就忽略；`LongTextView.tsx` 改用它 | 自查 + 独立对抗 review 一轮 + 复核一轮：P0 0 / P1 0 / P2 2 / P3 2，全部修复（回响保质期 200ms 太短、测试空洞、惰性初始化、注释）。P0/P1 清零 | `CHANGELOG.md`「长文对照：停手后两栏还在互相推着滚」；`sense/README.md` 测试数与范围；本文件 §2 测试数、§3 | `e3e5db6`（fix）`f15cf6a`（docs）+ 回填提交（`git log --grep="回填长文同步滚动"`） | 不涉及（无部署目标） | 提交前：`npm test` 118 passed、`npm run typecheck`、`npm run build`、`cargo test` 90 passed；7 种变异各自让用例变红；**真 WebKit（Swift WKWebView + mock）**：旧代码停手后 234 个事件、译文写完后 204 个事件、位置一路漂；修复后均为 0，平滑滚动 5 个场景精确到位。未覆盖：真机触控板惯性 / 橡皮筋、Retina、Tauri 原生窗口手感。提交后复验（HEAD `f15cf6a`，已跟踪文件无改动）：`npm test` 118、`npm run typecheck`、`npm run build`、`cargo test` 90 通过；真 WebKit 里流式翻译写完后 0 个事件、用户式滚动停手后 0 个事件（同一手势滚满 1211）；内置浏览器真实滚轮：左栏滚到 800，右栏落在期望的 537，停手后 0 个事件，截图两栏停住 | `git revert` 本次修复提交；无数据迁移 |
| 2026-10-28 | **本机更新到新版本**：把 `main` 的 `40ea4f3` 构建并安装到 `/Applications/SENSE.app`。已安装的是 2026-10-23 的构建，这次一并带上其后的全部修复（评审 P1 六项、P2 / P3 25 项、长文同步滚动回响）。未改任何代码与文档以外的东西 | 不涉及代码改动；构建内容在此前各行已 review | 本文件 §2.1（推送现状）、§3 | 无新提交（构建自 `40ea4f3`，已跟踪文件无改动） | 本机 `/Applications/SENSE.app`，已重启（`./tools/install-app.sh`，用 `SENSE Dev` 签名）；旧版备份在 `~/cpcsoft/SENSE-app-backups/SENSE-before-scrollfix.app` | 构建 `tauri build` 成功（release 1m28s，9.12 MiB）；安装后可执行文件时间为本次构建、`codesign --verify --deep --strict` 通过、`sense-ocr` 在位；dist 前端包里含新的回响判定；进程存活、统一日志无崩溃与报错。**未验证**：呼出窗口与快捷键（桌面控制授权被拒，需你按 `⌃⌥⌘Space` 确认一次）、重装后首次查词的钥匙串弹窗（点「始终允许」）、屏幕录制授权是否保留 | 退出 SENSE，`rm -rf /Applications/SENSE.app && cp -R ~/cpcsoft/SENSE-app-backups/SENSE-before-scrollfix.app /Applications/SENSE.app && open /Applications/SENSE.app` |
| 2026-10-30 | **中文输入先译成英文，再解析那个英文词**（用户需求）：中文输入时提示词在原提示词后追加「先选译词、再解析」一节；英文提示词与缓存 key 不变（英文指纹 `c29d39c6…3922d8` 改动前后一致）；缓存 key 里的提示词指纹按输入语言取，写缓存与标过期共用 `lookup_key`。中文输入拿回的词里还有汉字就算没译成：标废后以 0.2 重试一次，仍不行降级展示并在卡片上方明说，降级卡不进缓存；词是英文而 `lang` 照抄 zh 的不重试、归一成 en。卡片标题上方「充电 → 英文」。**契约：字段名不变、没加字段，但 `word` / `lang` 在中文输入时的含义偏离 spec 的字段说明（原词 / 查询语言）——负责人 2026-10-30 已确认接受** | 自查 + 独立对抗 review 一轮：P0 0 / P1 0 / P2 5 / P3 11。P2：①流式预览变最终卡片时 SenseCard 被重新挂载、已展开的义项合上（本次引入的回归，已修：预览与最终结果同一树形，有树形守卫测试）；②`lang≠en` 被当硬失败，英文词会被误判、横幅说错（已改成只看词本身，lang 归一成 en）；③测试缺口：变异实验存活接线 / key 一致性 / 汉字范围 / 降级路径竞态 / 横幅与「充电 → 英文」行（已补测试，TS 55 + Rust 16 个变异现无非等价存活）；④契约语义偏离（已写进 CHANGELOG，经负责人确认）；⑤提示词的「中文只出现在…」白名单会诱导 scene / domains 变英文（已改负面清单）。P3：预览先画中文卡、标题被截断、读屏、提交卫生 4 项已修，其余记入 CHANGELOG「已知局限」。P0 / P1 清零 | CHANGELOG「新增 · 2026-10-30 — 中文输入先译成英文，再解析那个英文词」、sense/README「现在能做什么」与测试数、本文件 §2 测试数、本行 | `4e42134` feat(lookup): 中文输入先译成英文，再解析那个英文词 | 本机安装见同日下一批部署行 | 该提交单独拉出来：前端 175 / Rust 95 个测试通过；真 WebKit（模型由 mock 提供）：「充电」→ 骨架屏 → 英文预览 → recharge 卡片，预览阶段展开的义项在流式结束后仍展开，第二次命中缓存，`UNTRANSLATED=1` 降级横幅在卡片上方、降级卡不进缓存。**没验证真实模型下的译词质量**（挑的词是否贴切、是否遵守「只给一个词」「scene 用中文」、重试率与降级率） | `git revert 4e42134`（中文词条缓存因指纹变化而重建，不用清库） |
| 2026-10-30 | **左上角红黄绿三颗灯 + 四边四角可拖拽缩放**（用户需求）：红 = 退出应用、黄 = 隐藏窗口、绿 = 放大到屏幕可用区域 / 还原（红黄的含义由负责人 2026-10-30 明确选定）；8 个把手覆盖窗口外缘到面板内 4px（系统自带的缩放区只有最外缘约 3px，可见面板比窗口缩进 12px 所以抓不到），前端算新外框、Rust 一次设好位置和大小；删除前端的 `core:window:allow-set-size` | 自查 + 独立对抗 review 一轮：P0 0 / P1 1（划词浮层在字号缩放下错位，见下一行）/ P2 3：①`sanitize_frame` 不夹坐标，\|值\| 逼近 2^31 时 AppKit 抛未捕获异常、整个进程退出（已修：坐标夹进 ±100000，加测试）；②拖拽时松手丢失（窗口被隐藏、右键菜单打断）会留下监听器（已修：抽出 `createDragSession`，移动事件里主键已松开 / 窗口失焦 / 指针捕获丢失都收尾，加单测）；③红灯 = 退出的风险（评审建议改成隐藏）→ 已问负责人，选定「红 = 退出、黄 = 隐藏」，风险写进 CHANGELOG 与 README。P3：`quit_app` 缺 `ensure_main`（已补）、注释里「没有系统缩放边」的错误前提（已按我在旧版上的实测改正：有，但只有最外缘约 3px）、绿灯两个边界与动画阻塞（与系统 zoom 同一语义，记入已知边界）等。P0 / P1 清零 | CHANGELOG「新增 · 2026-10-30 — 窗口左上角红黄绿三颗灯，四边四角可拖拽缩放」、sense/README、本行 | `19006f3` feat(window): 左上角红黄绿三颗灯，四边四角拖拽缩放 | 本机安装见同日下一批部署行 | 该提交单独拉出来：前端 207 / Rust 102 个测试通过；真实 Tauri 调试构建里读回外框（放大 / 还原 / 缩放 / 夹紧 / 菜单栏），旧版上实测系统缩放区宽度；真 WebKit（合成指针事件）：8 个把手几何、最小尺寸、右键 / 快速点击 / 慢 IPC / 松手丢失 / 失焦。**没验证**：3px 圈里的光标、角区大小、多屏 / Retina、窗口拖到屏幕外 | `git revert 19006f3`（同时恢复旧的右下角把手与 `allow-set-size`） |
| 2026-10-30 | **⌘+ / ⌘- / ⌘0 缩放界面字号**（用户需求）：给面板内容套 CSS zoom，70%–160%，每次 10 个百分点，记住上次的字号（`localStorage` 只存一个倍数，机密仍不进前端存储），到头提示；同时调整标题栏（标签不换行、提示放不下时截断） | 自查 + 独立对抗 review 一轮：P0 0 / **P1 1：划词浮层（`position: fixed`）在 zoom 子树里，字号 ≠ 100% 时错位、被窗口裁掉**（已修：倍数经 `UiScaleContext` 交给 `LongTextView`，`glossPosition` 在设备坐标里夹紧后除以倍数；真实长文视图里 4 档 × 3 个点击位置实测到位；加单测）/ P2 0 / P3：小键盘 `code` 兜底没被测试覆盖（已补）、三颗灯接线与把手零覆盖（已补 SSR 测试）。P0 / P1 清零 | CHANGELOG「新增 · 2026-10-30 — ⌘+ / ⌘- / ⌘0 缩放界面字号」、sense/README、本行 | `13c35b5` feat(ui): ⌘+ / ⌘- / ⌘0 缩放界面字号 | 本机安装见同日下一批部署行 | 前端 233 / Rust 102 个测试通过；真 WebKit 用合成 NSEvent：⌘= / ⌘⇧= / ⌘- / ⌘0 / 小键盘、没按 ⌘ 时打字不受影响、录制快捷键时不触发、130% 下同步滚动正常、重启后恢复、70% / 160% 版面。**没验证**：输入法候选框、Retina、非美式键盘布局 | `git revert 13c35b5`（已保存的字号偏好留在 `localStorage` 里无害） |
| 2026-10-30 | **更正上面 `19006f3` 一行的一处**：装到本机后用真实输入实测，发现 `createDragSession` 里「移动事件的 `buttons` 为 0 就当已松手」（评审 A1 的修法）会让合成鼠标事件下的把手拖不动——WebKit 的 `PointerEvent.buttons` 取自系统当前的按键状态，辅助功能 / 远程控制 / 自动化工具发的合成事件按下和拖动期间都是 0。改成只有按下那一刻就带着主键值才信后面的 `buttons`；补回归测试 | 自查（评审之后的新改动）：在 WKWebView 里用合成 NSEvent 复现并确认原因；变异测试（总是信 / 总是不信 `buttons`）两个方向都被杀；P0 / P1 无 | CHANGELOG（窗口一节「装到本机后用真实输入实测发现的 bug」）、本行 | `8519f7c` fix(window): 合成事件下边缘拖拽起不来——只有按下时就带着主键值才信 buttons | 同下一行 | 前端 234 个测试通过；装到本机后真机拖拽五条边、两个角、最小宽度夹紧都对（见下一行） | `git revert 8519f7c`（会让合成事件下的拖拽重新起不来） |
| 2026-10-30 | **本机更新到新版本**：把 `main` 的 `8519f7c`（含 `4e42134`、`19006f3`、`13c35b5`、文档提交 `cf8ae54`）构建并安装到 `/Applications/SENSE.app`，用 `SENSE Dev` 签名。第一次安装（构建自 `cf8ae54`）后发现把手拖不动的 bug，修复后又装了一次。旧版（构建自 `40ea4f3`）已备份在 `~/cpcsoft/SENSE-app-backups/SENSE-before-3features.app` | 不涉及代码改动；构建内容在此前各行已 review | 本文件 §3、CHANGELOG 里补了真机结果 | 无新代码提交（构建自 `8519f7c`，已跟踪文件无改动） | 本机 `/Applications/SENSE.app`，已重启（`./tools/install-app.sh`，用 `SENSE Dev` 签名）；旧版备份见左 | `codesign --verify --deep --strict` 通过；进程存活，统一日志无 error / fault，无崩溃报告。真机（computer-use 后台点击 / 拖拽 / 输入）：绿灯放大 → (0,30) 1920×960、再点还原 → (480,60) 960×900；黄灯隐藏（进程仍在）、`open -a SENSE` 能再呼出；红灯退出（进程退出）、重新打开正常；把手：系统缩放区（距外缘 2px）右 960→920，把手区（距外缘 10px）右 960→920、左 x 480→520 且宽 920→880、上 y 60→98 且高 900→862、下 862→822、右下角 880×822→840×782、左上角到 (560,138) 800×742，往里拖过最小宽度被夹在 760。真实模型（deepseek-flash）：「充电」→ recharge（备注「也可译 charge（口语）、top up（充值场景）」）、「落实」→ implement、「画蛇添足」→ overdo、「交代」→ explain，都带「→ 英文」行，流式阶段先骨架屏再出英文预览，第二次查「充电」命中缓存（0 请求）。Keychain 授权弹窗由负责人亲手点了「始终允许」。**未验证**：⌘+ / ⌘- / ⌘0 与全局快捷键 ⌃⌥⌘Space 在装好的应用里的真实按键（后台控制发不了这类按键，申请全屏控制没得到回应，需要你按一下确认）、输入法候选框、Retina、译词质量的统计 | 退出 SENSE，`rm -rf /Applications/SENSE.app && cp -R ~/cpcsoft/SENSE-app-backups/SENSE-before-3features.app /Applications/SENSE.app && open /Applications/SENSE.app` |
| 2026-10-30 | **截屏翻译：译文的位置、字号、背景色与原文一致**（用户反馈，附截图）：辅助程序读像素（底色 / 字色 / 笔画包围盒 / 允许遮盖的范围 / 右边的空白 / 逐词框；整图先转 sRGB；表格线、分隔线、色块边不算笔画；渐变底保守处理）；Rust 带着这些字段并换算成逻辑像素、多行合并、`unchanged` 标记、译文全部原样保留时放大覆盖层窗口；前端新增 `overlayLayout.ts`（纯函数排版：用逐词框宽反推原文字号并吸附、位置对齐、放不下先往右延伸再缩再截断并标出、底色字色取自原图）、覆盖层画层 + 错误边界 + IPC 边界清洗，译文与原文一样的块不画。**OCR 语言顺序（中文几乎读不出）是独立问题，本次未改**，已写进 CHANGELOG 的已知限制 | 两位独立对抗评审各两轮。辅助程序 + Rust：P0 无；P1 渐变底（译文旁露出原文尾巴）——已修；第二轮指出我这轮修复引入的 2 个回归（平色 + 软边被误判成渐变、高竖笔 / 下划线被当成线条）——已修并补用例；P2 线条判据、合并语义、`isFinite`、自检会被旧产物蒙混、变异存活——已修。前端：P0 无；P1 中灰底字色对比度、缺 `lines` 时多行被压成一行、数据缺字段时渲染崩溃留下关不掉的透明窗、提示条在小选区被裁——已修；第二轮：`blocks` 里混进 `null` 仍整树崩溃（错误边界 + IPC 边界清洗，评审员的真 WebKit 场景确认 Esc / 点击仍能关闭）、标点悬挂没有上限——已修。P0 / P1 清零 | `CHANGELOG.md` 增「修复 · 2026-10-30 — 截屏翻译：译文的位置、字号、背景色与原文一致」（原因、怎么改、验证、评审、已知限制、回滚）；`sense/README.md`「截屏翻译」与测试说明；本文件 §2 测试数（前端 325 / Rust 116）、§3 本行 | `2adee5e` feat(capture): 辅助程序读像素，量出底色、字色、笔画范围与逐词框，Rust 带给覆盖层 ｜ `973ae79` feat(capture): 译文原位替换——字号、底色、字色、位置与原文一致 ｜ `84f682a` docs: 记录截屏翻译的原位替换：CHANGELOG、README、§2 测试数、§3 记录 | 本行不涉及（本机安装见下一行） | 真应用（debug 构建 + `SENSE_DEV_CAPTURE_PNG` + 隔离 HOME + 本地 mock 翻译，截覆盖层窗口本身再与原图合成）对比：改动前复现用户截图里的现象，改动后黑块消失、各行字号一致；字号估计 76 块（4 张图，含 2 倍图按 Rust 规则离线验证）平均 0.54px、最大 2px（同一批块旧算法 1.84px、最大 12px）；底色与真值 0 处不符；左边界偏差 ≤1px；表格 32 格笔画框无一超出 Vision 框 3px；蓝色面板被改动的像素 152 → 0；前端 325、Rust 116、辅助程序自检全过；变异检验 Swift 约 40 / Rust 8 / 前端约 60 个，全部击杀（等价变异除外）；评审员压测 `analyzeLine` 约 400 万次随机输入 0 崩溃，平色 1500 例 / 渐变 1142 例与其验证过的修法逐例一致 | `git revert 973ae79 2adee5e` 及本次文档提交；无数据迁移；回滚后覆盖层回到固定深色底、按框高估字号 |
| 2026-10-30 | **本机更新到新版本**：把 `main` 的 `84f682a`（含 `2adee5e`、`973ae79` 与文档提交）构建并安装到 `/Applications/SENSE.app`，用 `SENSE Dev` 签名。此前已安装的是「三项改动」那版（构建自 `8519f7c`）。未改代码与文档以外的东西 | 不涉及代码改动；构建内容在上一行已 review | 本文件 §3 | 无新代码提交（构建自 `84f682a`，已跟踪文件无改动） | 本机 `/Applications/SENSE.app`，已重启（`./tools/install-app.sh`，用 `SENSE Dev` 签名）；旧版备份在 `~/cpcsoft/SENSE-app-backups/SENSE-before-overlay-fidelity.app` | release 构建成功（1m04s）；安装后可执行文件与 `sense-ocr` 的时间为本次构建、`codesign --verify --deep --strict` 通过；包里的 `sense-ocr --selftest` 通过，在侧栏 / 渐变 / 表格夹具上输出含 words / bg / fg / ink / cover（渐变那张有一行按设计走保守路径，不给 ink / fg）；`dist` 里含新逻辑（错误边界提示、「译文和原文一样」说明、格式错误提示）；进程存活，统一日志无 error / fault、无崩溃报告。**未验证**：已安装版本里真实的 `screencapture -i` 拖框与覆盖层——后台控制发不了系统选框的鼠标事件，发布版也关掉了 `SENSE_DEV_CAPTURE_PNG`；覆盖层的渲染是在 debug 构建（同一份代码）里用真窗口、真 OCR、真翻译请求路径验证的。需要你按 `⌃⌥⌘X` 拖一个框试一次；`⌘+` / `⌘-` / `⌘0` 与全局快捷键 `⌃⌥⌘Space` 的真机按键也仍待你确认（同上一次）；若重装后出现钥匙串弹窗，点不点「始终允许」是你的决定 | 退出 SENSE，`rm -rf /Applications/SENSE.app && cp -R ~/cpcsoft/SENSE-app-backups/SENSE-before-overlay-fidelity.app /Applications/SENSE.app && open /Applications/SENSE.app` |
