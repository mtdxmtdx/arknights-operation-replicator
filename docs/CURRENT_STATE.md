# 当前状态

更新时间：2026-07-27

## 当前目标

将开局暂停、普通暂停/恢复、暂停技能和暂停撤退交给用户已启动的 AFA，同时保留部署和自适应逐帧脉冲的 Rust 直注入；动作必须以派发后的新尺子样本确认目标帧和暂停状态。

## 已验证成果

- `repl-input` 已支持 XButton1/2、上下/左右滚轮的 Windows `SendInput`。
- AFA 适配器只读 Settings.ini，解析 `PressPause`、`ReleasePause`、`PauseSkill`、`PauseRetreat`，检查 `AutoBeginPause=1`、`DefaultStrongHoldProtocol=0`、AFA.exe 进程和管理员权限。
- Session 已移除 Rust 技能/撤退的三触控、Hover、固定长按猜测，改为定位鼠标后触发 AFA 热键。
- `PauseController` 的直接暂停/恢复入口已改名为 `diagnostic_pause()` / `diagnostic_resume()`，只由
  `step-test` 诊断工具使用；主运行路径保留 `pulse()` 并全部通过 AFA 委托普通暂停/恢复。
- Runner 已在 AFA 委托动作派发后等待 `frame_id > min_frame_id`；确认窗口中的运行态样本立即失败，
  0.2x 暂停只等待 1x 暂停，最终动作确认后不补输入。逐帧 pulse 的运行→暂停过渡仍由其自身逻辑处理。
- UI 已显示 AFA 就绪状态，开始前执行只读预检，编队确认后尝试一次恢复游戏前台。
- Runner 检测到战斗后不再发送开局 `PressPause`，只等待 AFA 自动开局暂停产生可信的
  `1x_paused`；状态机用独立的 `OpeningPauseDelegated`/`OpeningPauseIneffective` 语义确认零输入边界。
- 局部自动验证：`cargo test -p repl-input` 34/34、`cargo test -p repl-app` 20/20、`cargo check -p repl-app` 通过。
- `repl-core` 100/100 通过，包含开局零输入委托、AFA 未自动暂停中止和运行期 Pause 独立确认测试。
- 2026-07-27 已冻结并写入计划的设计约束：确认窗口拒绝一切运行态；`0.2x_paused` 只能过渡，
  最终必须是目标帧 `1x_paused`；同帧动作各自使用派发前 `min_frame_id`；确认循环位于 Runner；
  XButton/滚轮走鼠标事件；AFA INI 键名集中定义但必须通过首次实机日志验证。
- 只读核对同工作区 AFA 源码后，已确认键名来自 `src/lib/config.ahk`，发布工作流使用 `AFA.exe`；
  这不替代用户机器上的实际配置值和热键捕获验收。
- 当前阶段分支为 `develop`，已建立本地检查点并推送到 `origin/develop`。根目录旧二进制/日志和兼容修复前
  生成的日志不能作为本轮验收证据；实机证据来自 20:09 后构建的 `target\release\repl-app.exe`。
- 只读检查用户 `%APPDATA%\ArknightsFrameAssistant\PC\Settings.ini` 的六项验收配置均符合预期：
  `PressPause=g`、`ReleasePause=Space`、`PauseSkill=XButton2`、`PauseRetreat=XButton1`、
  `AutoBeginPause=1`、`DefaultStrongHoldProtocol=0`。
- 首次 M8 启动发现该实机 `Settings.ini` 是带 `FF FE` BOM 的 UTF-16 LE；原 UTF-8-only
  读取因此报“读取 AFA 配置失败”。读取器现按 BOM 支持 UTF-8、UTF-16 LE/BE，已用 LE/BE
  文件回归测试覆盖，且未修改用户的 AFA 配置。
- ACCEPTANCE 和 THIRD-PARTY-NOTICES 已同步 AFA 委托与严格确认语义；`repl-input` 测试现为 34 项。
- 全工作区自动验证已通过：`cargo test --workspace`、`cargo clippy --workspace --all-targets -- -D warnings`、
  `cargo build --release`、`cargo fmt --all`、`git diff --check`。

## 未完成/未验证

- M8 已开始但未稳定通过：当前构建曾连续 4 次在 Deploy=10、Skill=60、Retreat=180 收到目标帧
  `1x_paused` 并零输入结束；第 5 次在任何动作派发前，从开局第 0 帧恢复后第一条可信读数
  已到第 33 帧，越过首目标 10。此前同一构建还出现过两次运行期 Pause 临界失败和一次 Deploy
  确认落在第 11 帧；链路可工作，但成功率未达到验收要求。
- 第 5 次失败前尺子先报告 `not_in_battle`/不可信，说明 AFA 第 0 帧自动暂停会冻结开局标题和
  费用条渐显，恢复后存在尺子盲区。当前示例首动作已移到第 60 帧作为验收规避，但代码尚未
  识别或根治该盲区；运行期 Pause 的 4 样本临界窗口也尚未修复。
- ACCEPTANCE §5.4 的录像 CSV 10/10 核验尚未开始；M8 稳定前不得开始。

## 下一步

阶段性知识收尾后，下一步在当前分支分别处理：①开局 `not_in_battle` 尺子盲区和首目标安全策略；
②运行期 Pause 不应只按 4 条样本判死。修复时增加逐条帧/状态诊断，重新构建后用首动作第 60 帧
的示例重跑 M8；稳定通过后才开始 M9 的录像 10/10 核验。不得修改 AFA 配置或回退旧 Rust 时序。
