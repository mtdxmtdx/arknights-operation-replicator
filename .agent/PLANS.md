# 执行计划：运行期暂停确认修复

## 目标

修复逐帧推进阶段偶发漏掉暂停键后仍盲等 1 秒并发送脉冲，导致约 30 帧超跑、随后中止的问题。

## 上下文

- 代码入口：`crates/repl-app/src/runner.rs`；状态机：`crates/repl-core/src/machine.rs`。
- `SLOW_STEP_PAUSE` 为 1000ms；1x 速度下约 30 帧，日志中的 `+29/+31` 与其吻合。
- 旧契约把 `Completion::PauseSent` 当成游戏已暂停，导致尺子没有确认时仍可返回 `Command::Pulse`。

## 已完成

- [x] 添加回归测试，证明运行期 `PauseSent` 后不能直接得到 `Pulse`。
- [x] 增加 `ConfirmingPause` 阶段；只有可信尺子样本报告 `paused=true` 才允许逐帧推进。
- [x] 运行期暂停确认失败时，在 `FRAME_LEAD` 安全余量内有限重试；持续失败或越过目标则中止。
- [x] 脉冲前 1000ms 稳定窗口改为持续消费尺子样本；发现暂停丢失会取消脉冲并重新暂停。
- [x] 脉冲实际发送成功后才设置 `pulse_in_flight`，避免预检查失败污染整定器统计。

## 验证

- `cargo test --workspace`：通过。
- `cargo build --release`：通过。
- `cargo fmt --all`、`git diff --check`：通过。

## 下一步

使用最新 `target/release/repl-app.exe` 配合实机重复 `examples/sample-job.json`，重点观察目标帧 10、60、180 的暂停确认、重试和脉冲日志；验收结束后把实机结果补入 `ACCEPTANCE.md` 与本状态文件。

## 风险/未验证

- 当前尚未在本次会话中执行 Windows 实机验收；输入注入、尺子连接和游戏窗口状态仍需真实环境确认。
- 运行期重试按每次 4 条增长样本触发，最多重试 2 次；如暂停键本身存在更长的固定处理延迟，需要依据新日志调整常量。
