# 工具

工具把模型的请求转成一次有结果的操作。模型读取工具描述，发出调用；runtime 检查本次 Invocation 的工具集合，执行操作，再将结果送回下一轮上下文。

## 身份与调用名称

工具目录使用稳定的 `ToolId`。内置工具是 `builtin:<name>`，MCP 工具是 `mcp/<registration-id>:<name>`。Profile 的工具配置和运行记录使用这些 ID。

模型看到的名称由 Invocation 创建时生成。例如 `builtin:workspace.read_file` 对应 `workspace_read_file`。MCP 名称根据服务器和工具名生成。稳定 ID 用于记录和配置，模型名称用于这次调用，两者的对应关系保存在工具快照中。

Invocation 使用的工具快照包含描述、参数、名称映射及预算上限，按工具目录、Profile 和结束方式创建；输出修订沿用原快照。每个 Invocation 的调用计数独立，由 `ToolRequestGate` 检查后进入内置工具或 MCP 的执行路径。

合并调用减少模型往返，不绕过工具预算：外层调用和内部读取、等待、提交、结束操作均计入对应工具及 Invocation 总预算。`read_matches` 预留一次聊天读取；开场文件与产物读取按文件计数，`contextRefs` 按引用计数，调用内缓存不改变计数。读取批次先整体检查并预留额度，失败不产生部分读取证明。`wait`、`finish_task`、`finish` 与 `finalize` 在创建任务、写文件或提交前检查后续能力的额度，实际进入对应操作时计数；写入校验失败或宿主拒绝时，不扣除尚未执行的结束操作。预算随检查点保存，恢复不重置。

## 内置工具

下表使用源码中的工具名。参数以 [descriptor 定义](../../src-tauri/crates/tt-application/src/services/agent_tools) 和 `api.agent.tools.list()` 返回的 schema 为准。

| 工作 | 工具 |
| --- | --- |
| 读取聊天 | `chat.search`、`chat.read_messages` |
| 读取激活世界书 | `worldinfo.read_activated` |
| 查找和使用 Skill | `skill.list`、`skill.search`、`skill.read`、`skill.run_script` |
| 读取文件 | `workspace.list_files`、`workspace.search_files`、`workspace.read_file` |
| 修改文件 | `workspace.write_file`、`workspace.apply_patch` |
| 发布与结束 | `workspace.commit`、`workspace.finish` |
| 委派与交接 | `agent.list`、`agent.delegate`、`agent.await`、`agent.handoff`、`task.return` |
| 掷骰 | `dice.roll` |

`chat.search` 可设 `read_matches: true`，在同一次调用中读取所有返回的不同消息索引，要求本 Invocation 同时有 `chat.read_messages` 能力。它保留原搜索结果，并复用普通消息读取的宏展开、顺序、历史可见范围、消息数量和预览上限；原文已完整返回时不必再单独读取，被截断的消息仍需补页。读取失败明确返回错误并保留搜索结果，不把失败当成全文读取成功。默认关闭；适合明确需要原文的精确查询，需先筛选大量命中时继续用普通搜索。

聊天工具读取 Run 输入对应的历史范围；文件读取使用 1-based 行号，聊天消息索引使用 0-based。较长文本可以分段读取。完整工具集合会按 Profile 收窄，return-mode 子 Agent 使用 `task.return` 作为结束工具。

## 参数

一次调用携带一组具名参数。线上要么是 JSON object，要么是它的 JSON 字符串编码，「没有参数」在各家实现里有多种等价写法（字段缺失、`null`、`""`、`"null"`、`"{}"`）。`ToolArguments` 是唯一的换算位置：先剥掉字符串编码，再用同一条规则判断，因此两种编码得到同一个参数集合，全可选参数的工具可以直接无参调用。

既不是 object、也不属于上述空集合写法的内容（截断的 JSON、数组、标量）原样保留。runtime 在预算计入之后、分派之前拒绝这类调用，返回可恢复的 `tool.invalid_arguments` 并回显开头一段原文；工具本身只接收已经确定为 object 的参数。

各渠道回放统一使用对象参数：合法对象保持原值，非法参数回放为 `{}`，拒绝原因由配对的 tool error 承载。持久化仍保留非法参数原文。参数对象不接受字面成员 `$serde_json::private::Number` 或 `$serde_json::private::RawValue`，以免 Serde 私有标记将对象误转为数值或其他 JSON 值；字符串参数在解码时拒绝，原生对象响应在 HTTP／流事件解析前拒绝，不执行被改变含义的调用。普通字符串中出现该文本不构成违规。

Gemini 的旧式 OpenAPI `parameters` 不能无损表达联合引用、自由对象和任意 JSON 值。包含这些结构的 Agent 工具使用 generateContent 的 `parametersJsonSchema`（与 `parameters` 互斥）；Interactions 则在其 JSON Schema `parameters` 字段保留原 schema。参数值无需转成字符串，运行器仍执行原有权限和参数校验；端点须支持所用 schema 传输，不通过删去类型或约束静默降级。

## 结果如何进入下一轮

`AgentToolResult` 包含调用 ID、工具 ID、文本、结构化结果、错误信息和资源引用。runtime 把它作为 tool message 追加到模型上下文，并保存调用记录。

模型能够修正的请求错误也返回 tool result，例如参数不合法、读取范围不正确或文件已变化。存储和运行状态错误则向 Run 收尾流程传播。聊天中显示的正文通过工作区提交产生。

## MCP 与 Skill

MCP 提供外部工具。MCP Manager 管理服务器、发现目录和调用权限；Agent 使用已发现的工具目录，调用前由 MCP 服务确认权限。较长结果存为 `tool-results/` 下的可读文件，模型收到摘要和文件路径。配置与连接行为见 [MCP](../CurrentState/MCP.md)。

Skill 提供按需读取的工作方法、材料和脚本。它沿用相同的工具调用路径，知识包本身的组织方式见 [Skill](Skill.md)。

## 添加一个工具

先看相近的内置工具，实现它的参数描述和执行逻辑，再在 `BuiltinAgentToolRegistry` 注册。工具返回内容供模型继续工作；需要文件修改、提交或任务控制时，通过现有 `AgentToolEffect` 交给 runtime 记录和处理。

需要网络、文件或脚本引擎等外部能力时，沿用仓库的 port 与 adapter 边界。验证应从可观察的调用结果和副作用入手，见 [测试](TestingStrategy.md)。

- [registry.rs](../../src-tauri/crates/tt-application/src/services/agent_tools/registry.rs)：内置目录。
- [policy.rs](../../src-tauri/crates/tt-application/src/services/agent_tools/policy.rs)：Invocation 工具快照。
- [tool.rs](../../src-tauri/crates/tt-domain/src/models/tool.rs)：`ToolArguments` 的线上编码换算规则。
- [tool_request_gate.rs](../../src-tauri/crates/tt-application/src/services/tool_request_gate.rs)：调用检查与预算。
- [tool_execution.rs](../../src-tauri/crates/tt-application/src/services/agent_runtime_service/tool_execution.rs)：分派和记录。
