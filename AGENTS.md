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

- AFA 负责自动开局暂停、普通暂停/恢复、暂停技能和暂停撤退；Rust 只负责部署拖拽、
  自适应逐帧脉冲、鼠标定位和单次 AFA 热键触发。AFA 配置只读，失败时禁止回退旧时序。
- 开局只等待 AFA 和尺子，不发送 `PressPause`。第 0 帧暂停可能冻结标题/费用条渐显，恢复后
  第一条可信读数实测可到第 33 帧；当前验收作业首动作至少第 60 帧。此类巡航越过与
  `initial_gap_ms` 无关。
- 每个动作派发前记录 `min_frame_id`；确认只接受更大的新样本。任何运行态立即中止，
  `0.2x_paused` 只能等待，最终必须是目标帧可信 `1x_paused`。
- `wait_next` 必须传 Machine 当前帧游标；传 0 会错误复用缓存快照。
- 脉冲可经历 `1x_running → 1x_paused`；结算前禁止补发 Pause。运行期显式 Pause 也必须先确认。
- `Output`、`Finish` 和最终动作后的收尾都是零输入；`after_last_action` 只兼容解析。
- `PauseController::diagnostic_pause/diagnostic_resume` 仅供诊断工具；主路径只保留 `pulse()`。
- NCC 前先重采样到 1280×720。`heightType` 原始值与 MAA 枚举名相反，禁止按名称修正。

## 当前状态

现役快照见 `docs/CURRENT_STATE.md`，执行计划见 `.agent/PLANS.md`，实机步骤见 `ACCEPTANCE.md`。
M8 已证明链路可工作但仍不稳定；M8 稳定通过前不得开始录像 CSV 10/10 或宣称端到端完成。
