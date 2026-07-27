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

167 个单元测试，`cargo test --workspace`。clippy 无告警。

## 已知坑（不看注释就会踩）

- `wait_next` 的 `after_frame_id` 必须传状态机当前游标；传 0 会让缓存旧快照立即满足，
  主动轮询不启动，空脉冲便永远等不到新样本。
- 运行期 `PauseSent` 只表示暂停键已发送，不代表游戏已经暂停；必须经过
  `ConfirmingPause` 并收到 `paused=true` 的可信尺子样本，脉冲前的 1 秒窗口也会持续复核暂停。
- **技能 / 撤退必须先用三次触控完成“解暂停 → 点目标 → 重暂停”**，三次触控后
  等 1 秒再发功能键；不要把 50ms 按键 hold 塞进重暂停之前。短暂 0.2x 会被丢弃，
  第 32 条仍未恢复 1x 才中止。
- **NCC 没有尺度不变性**，截图必须先重采样到 1280×720 才能和 MAA 模板匹配。
- **`heightType`**：MAA 枚举名 `Highland=0, Floor=1` 与实际数据相反
  （`tile_road=0`，`tile_wall=1`）。代码用原始整数，不引用枚举，不要按名字改。

## 当前状态与下一步

验收节 1–4、6 实机通过（2026-07-26）。

第 5 节开局暂停已验通；绑定、技能/撤退、0.2x 误中止和中止刷日志均已修复。
当前刻意慢速：最后 8 帧脉冲前等 1 秒，目标帧注入前等 2 秒，选中后等 1 秒。
运行期暂停现在先由尺子确认，再进入这 1 秒稳定窗口；确认失败会在安全余量内有限重试，
不会把盲等 1 秒后的约 30 帧超跑当成一次脉冲。

**需要用当前 release exe 重跑 ACCEPTANCE.md §5.2–§5.4，并完成 10/10 录像/CSV
帧号核验；日志面板滚轮和滚动条拖动也仍需人工确认。**

验收全部通过后记录结果进 ACCEPTANCE.md，打 1.0 tag。
