# 当前状态

更新时间：2026-07-28 13:43:14 +08:00

## 当前目标

将开局暂停、普通暂停/恢复、暂停技能和暂停撤退交给用户已启动的 AFA，同时保留部署和自适应逐帧脉冲的 Rust 直注入；动作派发前后都必须以尺子为绝对帧唯一真源，不能使用滞后的 Machine 游标盲发或确认动作。

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
- 尺子确认 AFA 已完成开局暂停后，Runner 会零输入等待 1 秒，再进入编队绑定和后续操作。
- Runner 将动作前原有的 2 秒盲等改为尺子稳定屏障：持续消费新样本，最终只有最新状态仍为目标帧的
  可信 `1x_paused` 才允许派发；尺子已从目标 10 到 11 时会在部署/AFA 输入前中止。返回样本的
  `frame_id` 继续作为派发后确认水位。UI 运行期间也直接显示尺子 `total_elapsed_frames`，不再被
  Machine 的滞后游标覆盖。Session 完成部署识别/坐标准备后，还会在首个拖拽或 AFA 热键前做一次
  最终尺子复核，封住屏障结束到实际输入之间的准备时间竞态。
- Pulse 收尾不再接受单条 `paused`：观察到 `running` 时必须等后续 `paused`；未观察到中间运行态
  （空脉冲或尺子漏采）时必须连续收到两条新 `paused`。这避免脉冲前已进入尺子管线的旧暂停画面
  被误当作最终暂停，并记录 `pulse deferred/observed running/settled` 调试证据。
- 运行期 AFA Pause 确认不再按尺子样本条数判死，而是从派发帧计算真实 `elapsed` 增长；同一逻辑帧
  的重复 running 样本不消耗宽限。最多允许前进 6 帧（约 200ms），`FRAME_LEAD=8` 编译期保证仍
  留 2 帧安全余量。日志记录 AFA Resume/Pause 派发边界、pending advance 和最终确认。
- 运行期 AFA Resume 现由 `resume_in_flight` 建模：`ResumeSent` 后，在第一条可信 running 样本到来
  前拒绝新的 paused/unknown 积压样本，不更新 Machine 游标；命令生成器同时强制只返回
  `AwaitFrame`，从而禁止同一运行区间重复发送 `ReleasePause`。确认日志为 `runtime resume confirmed`。
- 局部自动验证：`cargo test -p repl-input` 34/34、`cargo test -p repl-app` 23/23、`cargo check -p repl-app` 通过。
- `repl-core` 103/103 通过，包含开局零输入委托、AFA 未自动暂停中止、运行期 Pause 重复样本、
  Resume 积压 paused 样本隔离和 `paused(旧) → running → paused(最终)` Pulse 收尾回归测试。
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
- 2026-07-28 的 10/30/60 诊断作业再次复现：部署期间尺子已显示 11，复刻器仍显示 Machine 游标 10，
  派发后才以 `elapsed=11` 确认失败。上述本地修复已由回归测试覆盖并完成 release 重建，但尚未经过
  用户实机复验；不能据此宣称 M8 已通过。
- 11:39:16 构建实机复验没有发出 Deploy，但在目标 10 的动作前屏障收到更新的
  `1x_running elapsed=10`，随后画面推进到 11。状态机能进入动作前屏障证明它此前已用一条旧
  `1x_paused elapsed=10` 提前结算最后一发 Pulse；本地已改为事务式/双暂停确认，尚待再次实机复验。
- 12:01:01 构建已在第 10 帧完成部署并确认，但部署后的运行期 Pause 在四条分析样本内以
  `PauseIneffective` 中止，游戏随后越过目标 30。根因是旧逻辑统计 `frame_id` 样本数而非实际
  `elapsed` 增长；本地已改为 6 逻辑帧宽限，尚待实机复验。
- 12:43:35 构建的最新实机日志显示每个 cursor 连续派发两次 Resume，累计发送约 28 次
  `ReleasePause`，随后一次 Pulse 从 33 跑到 63。根因是动作前等待期间积压的新 frame_id paused
  样本在 `ResumeSent` 后把 `self.paused` 重新置真；13:05:30 release 已加入 Resume running 回执
  屏障并完成自动验证，尚待用户实机复验。
- 第 5 次失败前尺子先报告 `not_in_battle`/不可信，说明 AFA 第 0 帧自动暂停会冻结开局标题和
  费用条渐显，恢复后存在尺子盲区。仓库示例当前保留 10/40/60 作为早帧时序回归；正式验收副本
  必须把首动作移到至少第 60 帧。代码尚未识别或根治该盲区；运行期 Pause 的 4 样本临界窗口已
  完成本地修复但尚未实机确认。
- ACCEPTANCE §5.4 的录像 CSV 10/10 核验尚未开始；M8 稳定前不得开始。

## 下一步

先用 2026-07-28 13:05:30 本地重建的 `target\release\repl-app.exe` 复验部署后的巡航。每个运行区间
只能出现一次 `dispatching AFA runtime resume`；随后可有若干
`runtime resume waiting for running state`，但必须由一条 `runtime resume confirmed` 结束，期间不能
再次 Resume。接近下一目标后只允许一次 Pause，并在 `runtime pause confirmed` 后才进入 Pulse。
若 Resume 仍成对出现或单次 Pulse 跨越大量帧，保存新日志并停止验收。稳定通过后才开始 M9 的
录像 10/10 核验；不得修改 AFA 配置或回退旧 Rust 时序。
