# SENSE

macOS 上的 AI 翻译器桌面应用：查词给完整义项卡，长文给逐处批注，截图给即时解析。不做聊天框。

**当前进度**：D1 / 15 —— 工程骨架与窗口已跑通，尚未接入模型。

| 想了解 | 看这里 |
|---|---|
| 怎么跑起来、怎么测 | [`sense/README.md`](sense/README.md) |
| 产品与工程方案 | `SENSE-v1.0/docs/SENSE-落地方案.md`（交接包本地材料，未入库） |
| 数据契约与 prompt | [`SENSE-v1.0/spec/`](SENSE-v1.0/spec/) |
| 变更记录 | [`CHANGELOG.md`](CHANGELOG.md) |
| **改代码前必读** | [`MAINTENANCE_RULES.md`](MAINTENANCE_RULES.md)、[`AGENTS.md`](AGENTS.md) |

## 一句话架构

Rust 管权限、密钥、网络与缓存，React 只负责渲染。**API Key 永不进入 WebView**，所有模型请求由 Rust 发起。

## 六条硬约束

1. API Key 只存 macOS Keychain，绝不进前端 state / localStorage / 日志；设置页只回显 `sk-****abcd`
2. 所有模型请求由 Rust 发起，前端不直连模型、不接触 Key
3. 不改 `SENSE-v1.0/spec/` 字段名，只能加可选字段
4. 不做后端、账号、云同步；v0.1 产品侧服务器成本为 0
5. 模型输出必须过 zod 校验；失败重试 1 次（温度 0.2），再失败降级并明确告知
6. 部分成功优先：先到 3 个义项就渲染 3 个

## 目录

```
TranslateApp/
├─ sense/            工程本体（Tauri 2 + React 19）
├─ SENSE-v1.0/       交接包：方案、契约、prompt、提案（只读参考；仅 spec/ 入库）
├─ MAINTENANCE_RULES.md  变更规范：六步流程、项目命令、执行记录
└─ AGENTS.md         AI 代理入口
```
