# 多 Agent 协作

多个 Agent 在同一个 Run 中工作。每个 Agent 的一次执行是一个 Invocation，有自己的 Profile、上下文和工具循环；文件保存在共享工作区中。

协作通过任务包传递目标和材料，通过结果包传回结论。它类似管道中的输入与输出，runtime 用任务记录连接两端，文件路径承载较大的材料。

## 委派与交接

两种协作方式共用 `AgentTaskRecord`，区别在于任务之后由谁继续：

| 方式 | 工具 | continuation | 后续工作 |
| --- | --- | --- | --- |
| 委派 | `agent.delegate` | `return_to_parent` | 子 Agent 返回结果，调用方继续 |
| 交接 | `agent.handoff` | `transfer_control` | 当前 Agent 退出，下一个 Agent 接手 |

任务记录保存调用方、接收方、目标 Profile、任务内容和状态。模型只需选择 Agent、描述任务并引用文件；Invocation ID 和任务文件路径由 runtime 管理。

## 委派一个任务

调用方用 `agent.list` 查看可用 Agent，然后交付任务：

```json
{
  "agentId": "reviewer",
  "task": {
    "objective": "检查人物动机是否前后一致，并将修改建议写入 summaries/review.md。",
    "draft": "output/main.md"
  }
}
```

`task.objective` 必填，`title` 可选，其他字段可以携带这项工作需要的材料。`agent.delegate` 返回任务 ID 后，调用方可以继续做其他事。需要结果时调用 `agent.await`；它支持等待一个完成、等待全部完成或只读状态。

`agent.delegate` 可设置顶层 `contextRefs`，由运行器把主控明确选定的资料冻结进任务，不经模型转述。支持 `{"kind":"workspace","path":"persist/state.json","pointer":"/scene"}`（pointer 可省略）、`{"kind":"chat","index":0}`、`{"kind":"worldInfo","ref":"worldinfo:书名#条目编号"}`。工作区来源须主控已有完整读取记录、SHA 未变且仍可见；JSON 子树保留值与类型，序列化格式可变。聊天和世界书沿用主控对应读取工具的来源、宏与可见范围，拒绝缺失来源和被截断预览。它们不是自动取当前提示词过滤后的历史，调用方必须核对来源是否含未采用草稿或其他不适合交付的材料。最多16个引用、合计96,000字符，超限不静默裁剪。正文作为资料呈现而非新增指令，不赋予子任务任何额外路径权限；裁决、知情边界和停止点仍由主控明确指定。`task.referenceMaterials` 是运行器保留字段，不接受调用者伪造。同次解析只加载一次可见根清单；连续引用同一文件的字段复用该文件及 JSON 解析结果，缓存仅保留一份文件，不跨调用或 Run。任务创建前重新检查各源文件 SHA，变化则拒绝交付，不能因缓存忽略冲突；引用顺序和所选内容保持不变。

顺序工作流可在 `agent.delegate` 设置 `wait: true`，复用等待逻辑在同一调用内取得结果；要求调用方有 `agent.await` 能力。设置 `readArtifacts: true` 可连同完成任务的文件视图一起取得，免去独立读取回合；异步流程也可在 `agent.await` 设置同一选项。读取沿用调用方权限和版本记录，有返回 SHA 时必须匹配，最多8个文件和96,000字符；预览不冒充完整读取。等待超时、取消或读取失败仍明确返回，调用方应查询已有任务，不盲目再委派。合并委派的回执保留已创建的 `taskId`；产物读取失败时仍保留 `tasks` 中的任务状态、摘要和产物引用，并在 `readError` 中说明原因。产物 `sha256` 可省略，提供时必须是64位十六进制字符串（不接受 null）；新任务在 `task.return` 时校验，旧任务的无效元数据在读取时返回可恢复错误，不把父 Run 标记成无法恢复的未知执行结果。

子 Agent 的最后一个 `workspace.write_file` 调用可带 `finish_task: true`：写入成功后运行器以该文件的路径和 SHA 生成任务结果并结束子任务，无需模型再调用一次 `task.return`。只适用于有 `task.return` 能力的 return-mode 子任务，必须是本轮最后一个工具调用；写入失败不返回完成，不增加子任务的路径权限，也不会发布聊天。写入与任务结果保存不是同一跨文件原子事务，交付中断需检查已保存产物后恢复，不重写冒充新产物。

需要报告失败、提出问题或返回其他结构时，子 Agent 仍使用 `task.return` 结束：

```json
{
  "summary": "发现两处动机衔接问题，修改建议已写入 summaries/review.md。",
  "artifacts": [{ "path": "summaries/review.md", "kind": "markdown", "role": "review" }]
}
```

runtime 保存结构化结果，并生成 `summaries/<workspace-key>-result.md` 供其他 Agent 读取。调用方收到摘要、文件引用及结果中的其他字段，随后决定如何使用。已完成结果也会在后续轮次送入调用方上下文。

委派任务使用后台执行和 `task.return` 结束方式，聊天提交由前台 Agent 负责。当前 return-mode 子 Agent 不再向下委派或交接。

## 交给下一个 Agent

适合把资料整理、起草、润色等阶段交给不同 Profile：

```json
{
  "agentId": "editor",
  "handoff": {
    "objective": "完成最后一轮润色并提交正文。",
    "contextSummary": "事实核对已完成，保留原有叙述视角。",
    "workspaceRefs": ["output/main.md", "summaries/review.md"]
  }
}
```

`handoff.objective` 必填，其余内容按任务需要填写。交接前先收齐当前 Agent 的委派任务。交接成功后，当前 Invocation 标记为 `transferred`，executor 在同一 Run 中启动接收方；接收方沿用已有文件和提交记录，通过 `workspace.commit`、`workspace.finish` 完成工作。

## 接收方看到什么

runtime 用目标 Profile 准备新的提示词和工具集合。初始聊天材料来自 Run 启动时冻结的输入，任务包通过 `agentTask` 进入提示词。调用方应在任务中写明目标、必要背景和文件路径。

共享工作区意味着双方看到同一路径的内容。Profile 决定各自能读写哪些目录；涉及同一文件的并行工作仍使用工作区的内容冲突检查。不同任务可以约定各自输出文件，再由调用方汇总。

Profile 的 `delegation` 配置控制是否可被调用、是否可委派或交接、调用来源与并发数量。返回式子任务由 scheduler 运行；Run 收尾或取消时，runtime 结束尚未完成的子任务。

## 查看与修改实现

Timeline 使用 Invocation 和 Task 记录展示关系，用日志显示每个 Agent 的过程。`readTaskDetail({ runId, taskId, includeResult })` 按 ID 读取任务及按需结果。

- [invocation.rs](../../src-tauri/crates/tt-application/src/services/agent_runtime_service/invocation.rs)：创建 Invocation 与 Task。
- [delegation](../../src-tauri/crates/tt-application/src/services/agent_runtime_service/delegation)：委派、等待、返回、交接及接收方提示词。
- [scheduler.rs](../../src-tauri/crates/tt-application/src/services/agent_runtime_service/scheduler.rs)：后台子任务与取消。
- [descriptors.rs](../../src-tauri/crates/tt-application/src/services/agent_tools/agent/descriptors.rs)：模型可见工具参数。
