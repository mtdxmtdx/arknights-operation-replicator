# Arknights Operation Replicator — AGENTS.md

## 项目定位

把带绝对逻辑帧 `frame` 的 MAA copilot 作业在《明日方舟》PC 客户端上帧精确复刻。
ArknightsCostBarRuler 通过 `ws://127.0.0.1:2606` 提供唯一帧真源。

## 构建与运行

```powershell
cargo build --release
# 游戏为高完整性级别；AFA、尺子和本程序均由用户以管理员身份启动
.\target\release\repl-app.exe
```

前置条件：尺子已校准；AFA `AutoBeginPause=1` 且验收热键有效；MAA `resource/` 可由
`REPLICATOR_MAA_RESOURCE` 或父目录发现。读取 `docs/CURRENT_STATE.md` 和 `.agent/PLANS.md`
后再修改；外部 AFA、尺子和游戏只由用户操作。

## 架构与验证

- Rust 2021 workspace；Slint 1.8 软件渲染。
- `repl-core`：作业、投影、状态机；`repl-frames`：尺子 WebSocket；`repl-input`：输入/AFA；
  `repl-vision`：NCC；`repl-capture`：WGC；`repl-app`：UI 与编排。
- 日志写到 exe 旁的 `replicator.log`；probe 必须走 WebSocket，HTTP 已知会被 RST。
- 完成改造后运行 `cargo test --workspace`、`cargo clippy --workspace --all-targets -- -D warnings`、
  `cargo build --release`、`cargo fmt --all -- --check` 和 `git diff --check`。

## 稳定约束

- AFA 负责自动开局暂停、普通暂停/恢复、1 倍速逐帧、暂停技能和暂停撤退；Rust 只负责部署拖拽、
  鼠标定位和单次 AFA 热键触发。`[Hotkeys]/33ms` 是主程序必需项，AFA 配置只读，失败时禁止回退。
- 用户必须先进入关卡并等待 AFA 自动暂停，再点击“开始复刻”；开始预检只接受可信、战斗内、
  带 `frame_id` 的第 0 帧 `1x_paused` 尺子样本。Runner 随后等待用户手动将游戏切回前台和新的
  `frame_id` 样本，不发送开局 `PressPause`，也不自动抢游戏焦点。
- 非零可信 `1x_paused` 只能走风险确认式“继续”：截止帧固定且包含边界，前缀动作标为“用户确认
  完成”，新 Session 按作业推导场上状态，最终接管不得越过下一动作帧。继续结果不能计入 M9。
- 作业首动作没有第 60 帧下限；仓库 `sample-job.json` 的 10/40/60 序列可用于精确链路验收。
  开局和巡航仍必须按尺子实际读数闭环，若接管或运行期 Pause 已越过下一动作则安全中止。
- 每个动作派发前的稳定期必须持续读取尺子；最终仅当最新样本仍是目标帧可信 `1x_paused` 才能
  派发，并用该样本记录 `min_frame_id`。确认只接受更大的新样本；任何运行态立即中止，
  `0.2x_paused` 只能等待，最终必须是目标帧可信 `1x_paused`。UI 绝对帧始终显示尺子值。
- `wait_next` 必须传 Machine 当前帧游标；传 0 会错误复用缓存快照。
- AFA 逐帧动作可经历 `1x_running → 1x_paused`；看见 running 后等后续 paused 才结算，未见 running 时
  至少连续两条新 paused 才能结算，单条 paused 可能是旧管线样本。结算前禁止补发 Pause。
  运行期显式 Pause 也必须先确认。
- 运行期 AFA Pause 的无效判定只按派发后的真实 `elapsed` 增长计算，不按分析样本条数；同帧重复
  running 样本不消耗窗口。最多允许增长 6 帧，且 `FRAME_LEAD` 必须仍保留至少 2 帧余量。
- 运行期 AFA Resume 是等待尺子 running 回执的事务；回执前拒绝新的 paused/unknown 积压样本，
  不更新逻辑游标，也不得重发 Resume 或插入其他输入。
- `Output`、`Finish` 和最终动作后的收尾都是零输入；`after_last_action` 只兼容解析。
- `PauseController` 和 `initial_gap_ms` 只保留给旧诊断/配置兼容，主复刻不得调用 Rust 直接脉冲。
- `afa-step-test` 可调用同一 AFA `33ms` 热键做最多 500 次诊断；主复刻与诊断均以尺子证据结算。
- NCC 前先重采样到 1280×720。`heightType` 原始值与 MAA 枚举名相反，禁止按名称修正。

## 当前状态

现役快照见 `docs/CURRENT_STATE.md`，执行计划见 `.agent/PLANS.md`，首次安装验收见
`ACCEPTANCE.md`，当前开发构建的验收只按 `docs/REAL_MACHINE_ACCEPTANCE.md`。M8 已于
2026-07-29 通过；用户于 2026-07-30 确认当前 AFA 逐帧构建完成 M9 录像与 CSV 10/10。风险继续不得计入。
