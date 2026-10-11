# 工作区

工作区保存 Agent 可以反复处理的文件。一次 Run 拥有一份工作区；同一 Run 中的 Agent 共享文件，各自按 Profile 获得读写范围。

## 文件放在哪里

默认 Profile 使用下列目录：

| 目录 | 用途 |
| --- | --- |
| `output/` | 准备提交的正文或其他输出，默认正文为 `output/main.md` |
| `scratch/` | 草稿和临时材料 |
| `plan/` | 普通计划文件 |
| `summaries/` | 摘要与子 Agent 结果的可读版本 |
| `persist/` | 本次运行的持久内容工作副本 |
| `tool-results/` | 工具结果及较长结果的可读版本，只读 |

模型用 `workspace.list_files`、`search_files`、`read_file` 寻找和读取材料，用 `write_file`、`apply_patch` 修改文件。路径相对于工作区，例如 `output/main.md`。替换已有文件和应用补丁时，工具使用读取记录和内容 SHA 检查冲突；发生冲突后重新读取，再决定如何修改。

`write_file` 必须在 `content` 与 `source_path` 中二选一。`source_path` 从本次 Invocation 已完整读取或创建、且 SHA 未变化的可见文件复制文本；目标仍须可写，覆盖已有目标仍检查其读取记录与 SHA。`create_only: true` 使用仓储的原子“不存在”写入条件，即使已经读过目标也不能覆盖它。复制和只新建均只支持 `replace` 模式。

```json
{"path":"output/baseline.json","source_path":"persist/state.json","create_only":true}
```

`apply_patch` 可以保留单次 `old_string`／`new_string` 调用，也可以使用 `edits` 数组一次提交同一文件的多个变更，不能混用两种形式。批次要求完整读取，按数组顺序在内存执行，后项可引用前项结果；所有匹配和 SHA 检查成功后只写一次，任一失败都不写部分结果。批次包含1–128项，中间及最终文本不能增长超过8 MiB与源文件大小中的较大者。

```json
{"path":"persist/state.json","edits":[{"old_string":"\"balance\":100","new_string":"\"balance\":80"},{"old_string":"\"tickets\":0","new_string":"\"tickets\":1"}]}
```

`write_file` 使用 `source_path` 时也可附带相同的 `edits`，先修正已读源文本再一次写入目标，不修改源文件，也不先发布未修正的版本。例如，主控读完后台原稿后，可以直接复制或附少量修正写入 `output/main.md`，不用在工具参数中重新生成全文。复制只返回路径、SHA与大小等回执，不回传整份源文本；文件写入仍遵循原有目录权限和版本检查，正文显示时机见下文。

JSON 文件可以在 `apply_patch` 中使用 `json_ops` 数组，或在 `write_file(source_path=...)` 中对来源执行后再写目标。支持 `add`、`replace`、`remove`、`test`，路径使用 JSON Pointer（`~0` 表示 `~`，`~1` 表示 `/`，数组 `/-` 用于追加）；`test` 递归比较 JSON 值：数值按任意精度十进制数值比较，`1`、`1.0`、`1e0` 相等，正负零相等；对象按键和值比较，数组仍要求顺序一致，不把字符串或布尔值转换为数值。比较不改写原文中的数值表示。`replace/remove` 要求目标存在，`add` 要求父级存在，不提供 move/copy，也不允许删除根文档。它是明确定义的操作子集，不是完整 JSON Patch／JSON Schema 实现。JSON 操作要求完整读取、检查源与目标版本，在内存中执行全部操作后格式化为缩进 JSON并一次写入；任一失败不落盘。源文档和工具参数中的 JSON 数值保留任意精度，不经 f64 舍入；修改某一路径不会改写其他字段的数值。`required_keys` 可同时使用，语法校验自动启用。不得与字符串 edits 混用；每批1–128项，操作数据上限8 MiB，结果仍受批次大小限制。

JSON 操作、`validate_json` 和 JSON 子树资料引用在解析前拒绝任何层级的字面对象成员 `$serde_json::private::Number` 或 `$serde_json::private::RawValue`（含 Unicode 转义写法），不把它们误认成数值或原始 JSON 标记；失败不改源文件、不创建复制目标、不交付变形后的资料。普通字符串中的同名文本不受影响，原文读取和不解析 JSON 的文本复制仍可保留原始资料。

`read_file` 累计同一 SHA 版本的完整行覆盖范围。分页可以乱序或重叠，只有无缺口地覆盖整份文件才标记 `fullRead=true`；读取结果仍只包含当前页。版本变化清空旧覆盖，单行被截断的页不计为完整行，恢复运行保留已确认覆盖。超长单行仍受预览限制，不能通过只读行首取得完整读取资格。

`write_file`（replace）及 `apply_patch` 可带 `validate_json: true`，在写入前解析完整候选文本；可再提供 `required_keys`（最多64个顶层字段名），非空列表要求根为对象且字段存在。失败不写入，成功回执包含 `jsonValidation`，不回传完整文档。这是 JSON 语法与字段存在性校验，不是完整 JSON Schema、字段类型、账目或剧情语义校验；未启用时保留普通文本工具行为。

工作区读取返回文件原文。脚本需要展开模板时，可以显式调用 `macros.render()`。聊天、世界书和 Skill 的读取由各自工具完成，保留原有数据来源。

## Run 与聊天的关系

磁盘数据位于数据目录的 `_tauritavern/agent-workspaces/`：

```text
agent-workspaces/
  index/
    runs/<run-id>.json
  chats/<workspace-id>/
    persistent-states/<state-id>/
      manifest.json
      persist/...
    runs/<run-id>/
      run.json
      manifest.json
      events.jsonl
      input/...
      invocations/...
      tasks/...
      agent-results/...
      model-responses/...
      tool-args/...
      tool-results/...
      output/...
      scratch/...
      plan/...
      summaries/...
      persist/...
```

`workspaceId` 由聊天种类和 `stableChatId` 派生。聊天文件名用于定位当前聊天，稳定身份用于关联历次运行。每次生成都创建新的 `runId`。

`manifest.json` 描述工作区目录与输出；`input/` 保存提示词、Profile 和持久内容的起点。Invocation、任务与模型响应等目录供 runtime 和详情 API 使用。

## 提交到聊天

模型调用 `workspace.commit` 时，runtime 读取指定文件并请求前端宿主保存。默认操作是替换本次输出楼层的正文；`append` 则将文件内容追加到本次输出。Host bridge 沿用 SillyTavern 的输出处理与保存流程，成功后把结果交回 runtime。

首次显式提交前，前台运行还会展示写作进度：流式 `content` 写入形成实时正文；使用 `source_path` 成功复制到 Profile 配置的正文路径时，即使开启流式，也会在该模型回合结束时把最终文件交给宿主显示。失败的复制和后台写作者不会借此发布正文，同批非文本状态写入也不会覆盖待显示的正文。若要先读正文再生成记忆参数，应将审核后的正文采用放在独立的模型输出中，再开始状态维护；这不是提前执行尚未完整返回的工具参数。首次显式提交成功后，后续聊天发布仍由显式 `workspace.commit` 控制。`workspace.finish` 仍要求前台至少完成一次显式提交。

`workspace.commit` 可显式设置 `finish: true`，把已确认提交后的结束步骤交给运行器执行，不需要再请求一次模型调用 `workspace.finish`。该调用必须是本轮最后一个工具调用，Invocation 必须允许结束且具有 `workspace.finish` 能力。先准备并确认所需正文及状态工作副本；宿主拒绝提交时不会结束，确认后沿用正常持久版本发布与恢复路径。默认 `finish: false` 仍可多次提交或继续工作。它不把正文保存和持久 metadata 更新变成跨宿主原子事务，后者失败仍需恢复原 Run，不重放已确认动作。

前台主控的 `write_file` / `apply_patch` 可对最终的 `persist/` 操作设置 `finalize: true`：它必须是该次模型输出中唯一的工具调用，拥有 commit/finish 能力、完整读取或采用过正文，且没有待处理子任务。最近一次已确认提交必须以 replace 交付该正文路径的同一 SHA 版本；append 片段、尚未发布的改稿以及缺少提交 SHA 的旧 checkpoint 均在状态变更前拒绝 finalize。此时可正常维护状态后显式 finish，或先把完整正文显式 replace 提交再使用 finalize；不能把最后一个 append 片段当作完整正文覆盖聊天。只在这项状态操作成功后，沿用显式正文提交、宿主确认及持久发布流程；写入或校验失败不提交、不结束。其他文件操作必须已在先前回合确认。它不替代事实判断或写后复核，需要继续检查时使用普通分阶段流程。若状态已写入而宿主拒绝提交，回执包含 `savedMutation`，应修复发布后重试 commit，不重复状态变更。文件写入、聊天确认及持久 metadata 保存仍非跨系统原子事务。

已确认的提交会保留，即使后续运行失败。正文已显示但持久 metadata 尚未保存时，Timeline 标记状态仍在保存；运行失败时提示正文保留、状态未保存。读取下一轮起点若遇到 `persistStateStatus: not_committed`，返回 `agent.persist_state_pending`，不跳过该回复去继承更老的版本；用户可恢复原 Run、重生成该回合，或明确选择从空状态重新建立记忆。模型、工具与文件处理的详细过程放在 Timeline；聊天消息保存正文、可见 reasoning 和关联 Run 的 metadata。

## 将内容带到下一次运行

`persist/` 的起点由 `persistBaseStateId` 指定。初始化时，仓储把对应持久版本复制到本次 Run；模型随后像处理普通文件一样修改它。

配置 `context.initialBaselineFiles` 时，运行器在首次模型请求前把所选可见的持久文件按原文保存为 `baseline/<原路径>`，例如 `baseline/persist/state.json`。该目录对模型只读，不发布为下一轮持久状态；恢复和修订不重新捕获，不用后来修改的工作状态覆盖起点。缺失源文件不凭空造基准；新开局首次建立状态仍需任务自己的初始化协议。若初始文件视图与基准 SHA 相同且完整，回执中的 `baseline.alreadyRead=true` 表示同一文本的阅读证明可直接用于基准复制；否则需明确读取基准，不能猜测其内容。

`workspace.finish` 收尾时发布持久版本，并将其 ID 写入已提交消息的 Agent metadata。首次完成或持久内容发生变化时创建新的 `persistent-states/<state-id>/`；修订未改变持久内容时复用上一版本。后续生成根据当前消息或 swipe 选择起点，因此不同候选可以保有各自的持久内容。前端负责选择版本，仓储负责保存版本。当前持久内容支持新增与修改。

聊天分叉会复制持久版本并使用新聊天身份。运行历史清理与持久版本清理分别处理：缩减旧 Run 的材料不会删除仍被聊天使用的持久内容。

## 保留与清理

运行历史分为核心记录和完整材料。较近的 Run 保留全部文件，较早的 Run 可以只留 `run.json`、日志和摘要，超过历史窗口的 Run 再整次删除。材料清理后，Timeline 仍能显示保留的事件，对应文件详情可能已不可读。

`api.agent.retention` 提供设置、预览和执行入口；自动清理默认关闭。操作参数见 [Agent API](../API/Agent.md)。

## 源码

- [workspace_policy.rs](../../src-tauri/crates/tt-application/src/services/agent_profile_service/workspace_policy.rs)：目录与 Profile 的对应关系。
- [workspace 工具](../../src-tauri/crates/tt-application/src/services/agent_tools/workspace)：文件读改与提交请求。
- [FileAgentRepository](../../src-tauri/crates/tt-adapter-storage-userdata/src/repositories/file_agent_repository)：路径、文件、持久版本与清理。
- [聊天提交桥](../../src/tauri/main/api/agent-chat-commit-bridge.js)：接入前端聊天保存。
