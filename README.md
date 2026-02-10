# StatusHub

StatusHub 是一个 Rust 编写的状态桥接程序。当前实现：

- 监听指定 Discord 用户在线状态变更
- 将状态变更推送到 webhook
- 原生支持 OpenClaw `/hooks/wake`，并提供通用 JSON webhook 模式

## 设计目标

- 模块化：`config`、`discord`、`event`、`webhook` 解耦
- 可扩展：后续可新增更多状态源或 webhook sender，而不影响现有模块
- 可运维：结构化日志、配置校验、错误上下文

## 前置要求

- Rust stable（建议 1.85+）
- Discord Bot Token
- Bot 已加入目标 Guild，并开启 **Presence Intent**（Privileged Gateway Intents）

## 快速开始

1. 复制配置模板：

```powershell
Copy-Item .\config.example.toml .\config.toml
```

2. 编辑 `config.toml`，至少填写：

- `discord.bot_token`
- `discord.user_id`
- `discord.guild_id`（建议填写）
- `webhook.url`
- `webhook.token`（若 webhook 要求鉴权）

3. 运行：

```powershell
cargo run --release -- --config .\config.toml
```

## Webhook 模式

- `openclaw_wake`：发送 payload `{ "text": "...", "mode": "now|next-heartbeat" }`
- `generic_json`：发送完整事件 JSON，适配任意 webhook 接收端

## 事件示例（generic_json）

```json
{
  "source": "discord.status",
  "user_id": 123456789012345678,
  "guild_id": 987654321098765432,
  "previous_status": "offline",
  "current_status": "online",
  "observed_at": "2026-02-10T01:35:20.123456Z"
}
```
