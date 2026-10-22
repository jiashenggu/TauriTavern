# Profile 与预设

Profile 描述一个 Agent 如何工作：使用什么提示词和模型，能调用哪些工具，能读写哪些文件，以及如何交付结果。Preset 负责提示词的组织，LLM Connection 负责连接信息。

## 从默认配置开始

Agent System 的 Profile 面板可以复制和编辑配置。`Default Writer` 使用当前聊天的预设与模型，以 `output/main.md` 作为正文，支持工作区工具、Skill 和委派。内置默认配置和新建 Profile 默认开启流式传输；已保存配置继续使用原有设置。先用它完成一次运行，再按任务需要修改配置。

扩展也可以读取默认配置后另存一份：

```js
const agent = window.__TAURITAVERN__.api.agent;
const { profile } = await agent.profiles.load('default-writer');
const reviewer = structuredClone(profile);
reviewer.id = 'reviewer';
reviewer.displayName = 'Reviewer';
reviewer.instructions.agentSystemPrompt = '检查草稿中的人物动机，给出具体修改建议，并按任务要求返回结果。';
reviewer.delegation.callable = true;
reviewer.delegation.allowAsSubagent = true;
await agent.profiles.save(reviewer);
```

配置保存后，可通过 `profiles.diagnose('reviewer')` 查看模型、预设或工具引用的问题。

## 选择提示词和模型

两种绑定各自独立：

| 配置 | 选择 | 含义 |
| --- | --- | --- |
| `preset.mode` | `currentPromptSnapshot` | 使用当前生成准备好的提示词 |
| | `ref` | 用 `preset.ref` 指定的预设重新组装 |
| | `none` | 使用输入提示词，作为兼容绑定保留 |
| `model.mode` | `currentPromptSnapshot` | 使用本次输入冻结的模型连接 |
| | `connectionRef` | 使用指定 LLM Connection 和 `modelId` |
| | `requiresConfiguration` | 等待用户在本机选择模型 |

例如，使用专门的写作预设和本地连接：

```json
{
  "preset": {
    "mode": "ref",
    "ref": { "apiId": "openai", "name": "Writer Preset" }
  },
  "model": {
    "mode": "connectionRef",
    "connectionRef": "writer-model",
    "modelId": "model-name"
  }
}
```

这是 Profile 中的绑定片段。`writer-model` 需要是本机已经保存的连接。独立预设使用 OpenAI/chat-completion Preset，组装方式见 [Prompt assembly](PromptAssembly.md)。

Profile 面板的“预设与模型”支持选择“独立连接 / 模型”：从 LLM 连接目录选择已有连接，并直接填写完整模型 ID；主控与 SubAgent 均可编辑，内置默认 Profile 仍需先复制。连接目录只读取摘要，不展示凭据。切换连接保留当前模型 ID，须确认目标接口及连接参数兼容；保存只更新原生连接引用和 Profile 的模型 ID，不修改该原生连接本身。目录加载失败或当前引用未列出时保留原值并显示提示，不自动换成其他连接；可刷新目录重试。

原有“已保存模型”仍可作为快捷方式，同时填入连接与模型 ID，保存时按原规则物化 Model Target 对应的 LLM Connection。随后手工改模型 ID 不会自动被 Target 的旧模型名覆盖。连接的端点、凭据、路由与参数覆盖沿用对应连接，更新供后续解析使用，已准备好的调用不被改写。导出或嵌入到角色卡、预设时，本机的独立模型绑定会改为 `requiresConfiguration`，导入后重新选择即可。

## 调整工作方式

| 字段 | 用途 |
| --- | --- |
| `instructions.agentSystemPrompt` | Agent 的工作指令；省略时使用默认指令 |
| `context.initialChatHistoryMessages` | 初始历史楼数：`-1` 不主动裁剪，`0` 不注入，正数取最近 N 楼；仍受模型上下文预算限制 |
| `context.includeActivatedWorldInfo` | 是否在初始提示词中包含已激活世界书 |
| `context.initialWorkspaceFiles` | 可选的开场文件路径列表，最多8个；默认空列表，不自动注入文件 |
| `context.initialBaselineFiles` | 可选的持久基准文件列表，最多8个可见 persist/ 文件；默认关闭，在 Run 初始化时保存到只读 baseline/ |
| `context.eventArchive` | 可选的无损事件归档策略 `{ maxRecentEvents, keepRecentEvents }`；省略或 null 时关闭，可在高级 JSON 配置 |
| `tools.allow` / `deny` | 可用工具，deny 优先 |
| `tools.maxRounds` / `maxCallsPerRun` / `maxCallsPerTool` | 每个 Invocation 的轮数与调用预算 |
| `tools.toolDescriptions` | 替换模型看到的工具或参数描述 |
| `skills.visible` / `deny` | 按名称选择可用 Skill |
| `workspace.visibleRoots` / `writableRoots` | 文件读写范围 |
| `run.presentation` | 前台聊天输出或后台文件处理 |
| `run.stream` / `modelRetry` | 流式预览与模型请求重试 |
| `run.directRunnable` | 是否出现在直接运行的 Agent 选择列表 |
| `output.artifacts` | 输出文件；当前正文目标为 `messageBody` |

`initialWorkspaceFiles` 仅在新 Invocation 的首次模型请求前加载，沿用本 Invocation 的文件权限、行预览上限和读取记录，并要求可用的 `workspace.read_file` 能力。提供给模型的是带 SHA/fullRead 的资料消息，不是系统指令；完整视图可直接用于后续冲突检查，部分视图仍需补读。缺失文件明确报告未找到，权限和其他错误停止加载；合计资料正文上限96,000字符。恢复运行不重复注入，旧 Profile 省略此字段时行为不变。可在高级 JSON 中配置，例如 `"initialWorkspaceFiles": ["persist/state.json"]`。

工具字段使用稳定 ID，例如 `builtin:workspace.read_file` 或 `mcp/<registration-id>:<tool-name>`。模型看到的调用名称由 runtime 生成，见 [工具](ToolSystem.md)。

`plan/` 可以保存普通计划文件。当前运行器支持的 Profile plan 配置是 `mode: "none"`、空 `nodes`，尚无节点式工作流执行器。

## 无损事件归档

照微主控默认配置 `"eventArchive": { "maxRecentEvents": 32, "keepRecentEvents": 16 }`。策略须满足 `0 < keepRecentEvents < maxRecentEvents <= 4096`；其他 Profile 省略此选项时行为不变。它处理 `persist/state.json` 的 `recentEvents` 与 `archives` 数组，Profile 必须允许读取和写入状态及 `persist/history/`。

启用该策略的 Profile 必须使用说明这两个顶层数组的记忆协议，例如配套「照微·Agent」预设；仅开启 Agent Mode 不会把普通预设转换成 Agent 记忆协议。可以为 Profile 绑定独立的「照微·Agent」预设，避免当前聊天仍选择普通预设时漏掉协议。

状态写入统一使用 `persist/state.json`；启用归档时拒绝该路径的大小写变体，避免 Windows 上同一文件绕过校验，也避免其他平台创建大小写不同的状态副本。追加写入同样不允许，须通过完整替换或补丁更新 JSON。

模型写入 `persist/state.json` 时，无论直接替换、复制还是补丁，运行器都会在实际写入前使用与新 Run 初始化相同的结构校验：状态须为JSON对象，两个字段均为数组；每个近期事件须有非空字符串 `id`（最多512字节），且不能与其他近期事件或托管归档索引中的ID重复。托管索引的ID列表、路径和SHA格式也须有效。所有这些校验均不依赖事件数是否达到归档阈值；失败作为可修正的 `agent.event_archive_invalid` 工具错误返回，保留原文件，模型可在当前 Run 内修正，不等到下一轮才发现。此处检查状态内容，归档文件的实际内容与SHA仍在归档准备阶段核对。未启用归档的 Profile 不受这个特定状态结构约束。

新 Run 在初始化继承工作副本之后、建立只读 baseline 和发出首次模型请求之前检查事件数量。超过阈值时保留原数组末尾的指定数量，前部事件对象完整写入按内容 SHA 命名的历史 JSON，再确认文件内容和 SHA，最后带原状态 SHA 更新状态及索引。全部事件的未知字段和任意精度数值保留；不根据事件编号或剧情日期猜测排序，不做模型摘要，不修改人物、资源、任务等其他字段，也不增加剧情 revision。状态输入、整理后的状态和单批归档各限8 MiB。

已有存档缺少这些数组时，切换预设不会修改旧状态；下一次初始化仍会拒绝该起点。修复需保留原事件及未知字段，通过状态编辑接口建立新持久版本并更新聊天引用，不能用空数组替代尚未迁移的历史，也不直接覆盖旧持久版本。

运行器生成的索引以 `kind: "agent.eventArchive"` 标识，包含路径、SHA、事件数量、完整 ID 列表、参与者和 `firstListedTime` / `lastListedTime`；时间仅对应原数组中的位置，不宣称是排序后的时间范围。触发下一次归档时会核对已托管索引与文件的 SHA、格式及 ID，缺失、损坏、重复 ID 或路径冲突会停止初始化，不猜测合并或覆盖。其他已有索引保留原样。归档文件确认成功之前不会缩短状态；状态冲突时至多留下未引用的工作文件，不修改旧版本。

`memory_events_archived` 事件记录工作副本整理结果，不等于已发布。新布局与历史文件仍经正常持久版本提交一并发布，原版本及其他分支不改。baseline 冻结整理后的运行起点；恢复和 `/fix` 不重复启动归档，也不把归档当成新一轮结算。关闭选项不会删除已存在的归档，亦不会自动把旧事件搬回常驻状态。这优化常驻输入，不保证磁盘占用或每轮耗时同比减少。

## 配置协作

调用方开启 `delegation.canDelegate` 或 `canHandoff`，并允许相应工具。接收方开启 `callable`，再按用途选择 `allowAsSubagent` 或 `allowAsHandoffTarget`；`allowedCallers` 指定可调用它的 Profile，`["*"]` 表示所有调用方。

`descriptionForAgents` 用于向其他 Agent 介绍它适合处理什么工作。并发、任务数量和交接深度也在 `delegation` 中设置。完整流程见 [多 Agent 协作](SubAgent.md)。

## 源码

Profile 以 JSON 保存到 `_tauritavern/agent-profiles/profiles/`，当前 schema 版本为 3。

- [profile.rs](../../src-tauri/crates/tt-domain/src/models/agent/profile.rs)：字段与默认值。
- [agent_profile_service](../../src-tauri/crates/tt-application/src/services/agent_profile_service)：默认配置、解析和验证。
- [profile-model.ts](../../src/scripts/extensions/agent-system/src/profile-model.ts)：Profile 编辑与可移植配置。
- [LLM Connection API](../API/LlmConnections.md)：连接的创建与管理。
