# StatusHub

StatusHub 是一个 Rust 编写的状态桥接程序。当前实现：

- 监听指定 Discord 用户在线状态变更
- 将状态变更推送到 webhook
- 原生支持 OpenClaw `/hooks/wake`，并提供通用 JSON webhook 模式
- 可选：检测 Steam 游戏活动并附加游戏简介
- 可选：自定义 webhook `text` 的头部/尾部提示词
- 内置：Steam 信息内存缓存（TTL + 容量控制）
- 可选：通用 SQLite 数据库缓存（命名空间键值模型，不限于 Steam）
- 内置：持久化状态缓存（重启后可恢复上次状态）
- 可选：按固定间隔重复发送状态提醒（可设仅 Steam）

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
- `discord.emit_on_activity_change`（建议 `true`）
- `webhook.url`
- `webhook.token`（若 webhook 要求鉴权）

3. 运行：

```powershell
cargo run --release -- --config .\config.toml
```

## Webhook 模式

- `openclaw_wake`：发送 payload `{ "text": "...", "mode": "now|next-heartbeat" }`
- `generic_json`：发送完整事件 JSON，适配任意 webhook 接收端

## 可选功能

### 1) 自定义 text 头尾

在 `config.toml` 中配置：

```toml
[message]
prefix = "[系统事件]"
suffix = "请根据以上信息执行自动化流程。"
```

### 2) Rich Presence 变化触发

在 `config.toml` 中配置：

```toml
[discord]
emit_on_activity_change = true
```

说明：
- 开启后，状态不变但 Rich Presence 变化（如游戏、VSCode Presence）也会触发 webhook
- OpenClaw `text` 会附加 `Activity / Details / State`，便于观察 Rich Presence 内容

### 3) Steam 游戏信息增强

在 `config.toml` 中配置：

```toml
[steam]
enabled = true
api_key = "YOUR_STEAM_WEB_API_KEY"
language = "schinese"
description_max_chars = 240
timeout_seconds = 8
memory_cache_ttl_seconds = 1800
memory_cache_capacity = 512
db_cache_ttl_seconds = 86400
```

当检测到 Discord 活动里存在 Steam app id（如 `steam:570`）时，会调用 Steam appdetails API 获取游戏名和简介，并附加到 OpenClaw webhook 的 `text` 中。若配置了 `api_key`，还会额外获取当前在线人数。

### 4) 可选数据库缓存（通用）

```toml
[cache]
backend = "sqlite"
sqlite_path = "./data/statushub-cache.sqlite3"
```

说明：
- `backend = "none"` 时禁用数据库缓存
- `backend = "sqlite"` 时启用通用缓存服务（当前用于 Steam 数据与状态缓存，后续模块可复用）

### 5) 持久化状态缓存

```toml
[state_cache]
enabled = true
path = "./data/status-state.json"
```

说明：
- 启动时会恢复目标用户上一次状态，避免重启后重复触发错误状态变化
- 写入文件的同时，如果开启了 SQLite 缓存，也会同步写入数据库

### 6) 周期性提醒（30m / 1h / 1.5h ...）

```toml
[reminder]
enabled = true
interval_minutes = 30
steam_only = true
check_interval_seconds = 30
```

说明：
- `enabled=true` 后，状态持续超过间隔会重复提醒
- `interval_minutes=30` 时，会在 30m、60m、90m... 推送
- `steam_only=true` 时，仅在检测到 Steam 游戏活动（有 app id）时触发
- 提醒消息会在 `text` 中附带已持续时长（elapsed）与提醒序号

## VSCode Presence 排查

如果你使用 VSCode Discord Presence 插件但没有触发：
- 确认 `discord.emit_on_activity_change = true`
- 确认 Bot 已启用并使用 `Presence Intent`
- 确认目标账号在 Discord 用户设置中开启了活动展示（Display current activity）
- 确认插件成功连接 Discord RPC（本机 Discord 客户端在运行）
- 用 DEBUG 日志查看网关是否收到了活动：

```powershell
$env:RUST_LOG="statushub=debug,serenity=warn"; cargo run --release -- --config .\config.toml
```

你会看到 `received presence update` 日志，其中 `activities_count` 和 `activities` 会打印原始活动内容。

## 事件示例（generic_json）

```json
{
  "source": "discord.status",
  "user_id": 123456789012345678,
  "guild_id": 987654321098765432,
  "previous_status": "offline",
  "current_status": "online",
  "activity": {
    "name": "Dota 2",
    "details": "In Match",
    "steam_app_id": 570
  },
  "observed_at": "2026-02-10T01:35:20.123456Z"
}
```
