# Arknights Operation Replicator — AGENTS.md

## 一句话定位

把带绝对逻辑帧 `frame` 的 MAA copilot 作业在《明日方舟》PC 上帧精确复刻。
帧数唯一真源是 ArknightsCostBarRuler，通过 `ws://127.0.0.1:2606` 提供。

## 怎么跑起来

```powershell
cargo build --release
# 必须以管理员身份运行（游戏以高完整性级别运行，UIPI 会静默丢弃非提权进程的输入）
.\target\release\repl-app.exe
```

前置条件：
1. 游戏运行
2. `ruler-app.exe` 已校准；关闭时必须从托盘退出，否则旧进程占用 2606，复刻器会读到静止快照
3. `$env:REPLICATOR_MAA_RESOURCE` 指向 MAA `resource/`，或父目录可找到 `MaaAssistantArknights\resource`

## 技术栈

- **Rust 2021**，Cargo workspace，6 个 crate
- UI：**Slint 1.8**，`renderer-software`（不用 Skia——prebuilt 与 MSVC STL 不兼容）
- 截图：Windows Graphics Capture 按需截图；输入：`InjectTouchInput` / `SendInput`
- 帧源：WebSocket 客户端，有 waiter 时每 2ms 轮询 `getSnapshot`
- 识别：纯 Rust NCC 模板匹配，参考帧必须重采样到 1280×720（MAA 模板在此尺度裁的）

## 目录与约定

```
crates/
├─ repl-core/    作业模型、地图投影、坐标映射、帧复刻状态机
├─ repl-frames/  尺子 WebSocket 客户端（probe 必须走 WS，HTTP 会被 RST）
├─ repl-input/   触控注入、键盘、游戏内键位、逐帧脉冲（全程 spin_sleep，不用 Sleep(1)）
├─ repl-vision/  部署栏 NCC 识别
├─ repl-capture/ WGC 窗口截图
└─ repl-app/     Slint UI、配置、编排；日志写到 exe 旁边的 replicator.log
```

168 个单元测试，`cargo test --workspace`。clippy 无告警。

## 已知坑（不看注释就会踩）

- `wait_next` 的 `after_frame_id` 必须传状态机当前游标；传 0 会让缓存旧快照立即满足，
  主动轮询不启动，空脉冲便永远等不到新样本。
- 脉冲后的 `1x_running` 是 `ESC → pauseBattle` 之间的正常中间态；必须等更新的
  `1x_paused` 才结算脉冲，期间禁止补发 Pause。显式 Pause 也必须经尺子确认后才能开始 1 秒等待。
- **技能 / 撤退必须先用三次触控完成“解暂停 → 点目标 → 重暂停”**，三次触控后
  等待 100ms 让重暂停边沿落地，再 `key_down` 技能键 / 撤退键并保持 950ms 后
  依次执行无接触 `TouchInjector.Move`、鼠标移动到目标、等待 50ms，最后 `key_up`。
  AFA 不检查 Move 返回值，因此 Move 在 Windows 上失败时只记警告，仍须继续 MouseMove、
  50ms 和松键。当前实机上 Move 稳定返回 `0x80070057`；动作日志仍会成功，但这套时序
  **尚未实现“最后操作后游戏保持暂停”**。不要把日志成功或零输入收尾写成目标已达成。
  短暂 0.2x 会被丢弃，第 32 条仍未恢复 1x 才中止。
- **动作完成后的输入边界必须严格**：Deploy 以拖拽结束，Skill 以技能键结束，Retreat
  以撤退键结束。若没有下一动作立即收尾，不再等帧、补暂停、恢复或点击空白处；旧作业的
  `after_last_action` 仅兼容解析，不再改变行为。
- **NCC 没有尺度不变性**，截图必须先重采样到 1280×720 才能和 MAA 模板匹配。
- **`heightType`**：MAA 枚举名 `Highland=0, Floor=1` 与实际数据相反
  （`tile_road=0`，`tile_wall=1`）。代码用原始整数，不引用枚举，不要按名字改。

## 当前状态与下一步

验收节 1–4、6 实机通过（2026-07-26）。

第 5 节未通过。开局暂停、绑定、逐帧到达和动作注入可以跑通；2026-07-27 最新标准
release 连续两次完成 deploy → skill → retreat，Hover 均报 `0x80070057` 后继续，最终日志
到达“全部动作已注入”。但用户实机确认：最后技能或撤退后游戏没有保持在最后目标帧的暂停态。
此前尝试过普通 50ms `key_tap`、AFA 的约 1 秒长按、best-effort Hover/MouseMove，以及
`Command::Finish` 补暂停；均未达成目标，当前 `Finish` 保持零输入，问题明确标为未解决。
当前刻意慢速：最后 8 帧脉冲前等 1 秒，目标帧注入前等 2 秒；三触控选中后等
100ms，功能键按住 950ms，再执行 Hover、MouseMove、等待 50ms 后松开。

**下一阶段应先为“最后操作后保持暂停”建立可观察的实机反馈环并查明游戏实际状态；
解决前不要开始 ACCEPTANCE.md §5.4 的 10/10 帧号核验。日志面板滚轮和滚动条拖动
也仍需人工确认。**

验收全部通过后记录结果进 ACCEPTANCE.md，打 1.0 tag。
