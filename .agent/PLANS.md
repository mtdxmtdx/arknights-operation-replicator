# AFA 委托操作重构计划

## 目标与成功标准

把开局暂停、普通暂停/恢复、暂停技能和暂停撤退交给用户已启动的 AFA；复刻器只保留部署拖拽、
逐帧脉冲、鼠标定位/停靠以及 AFA 热键触发。每个真实输入动作都必须在派发后的新尺子
样本中确认仍停在目标绝对帧，最后一个动作确认后以零输入结束。

端到端成功的必要条件：AFA 可用性和权限预检通过；派发动作不会落入运行态；同帧动作
各自等待自己的新样本；最后动作完成后游戏画面和尺子都显示目标帧的 `1x_paused`；失败
时中止且不盲目重发输入。

## 当前上下文

- 项目根目录：`D:\Arknights Operation Replicator\replicator`。
- 帧数唯一真源是 ArknightsCostBarRuler；AFA 是外部、由用户管理的进程。
- AFA 配置只读：`%APPDATA%\ArknightsFrameAssistant\PC\Settings.ini`。
- 只读核对同工作区的 AFA 源码后，`src/lib/config.ahk` 确认四个键名及默认值，发布工作流确认
  二进制名为 `AFA.exe`；这证明了接口命名，但不能替代用户机器上的实际 INI/热键验证。
- 当前代码已经完成主要委托链路和自动测试；外部程序始终由用户操作。用户实机日志已经证明
  链路可完成全流程，但也暴露开局尺子盲区和运行期 Pause 临界时序。
- 保护用户已有改动；本任务不修改 AFA 配置、不自动启动外部程序、不回退旧 Rust 技能/撤退时序。

## 已冻结的设计决定（2026-07-27）

### 1. 输入所有权

- AFA 负责：自动开局暂停、普通暂停、普通恢复、暂停技能、暂停撤退。
- Rust 负责：部署拖拽、自适应逐帧脉冲、鼠标定位/停靠、发送一次 AFA 热键。
- 开局检测到战斗后，复刻器不发送 `PressPause`，只等待尺子确认 AFA 已停在可信的 `1x_paused`；
  `AutoBeginPause` 必须为 `1`。AFA 未自动暂停时中止，不补发开局热键。
- AFA 不可用、退出、降权、配置指纹变化或游戏失焦时，复刻器失败关闭；禁止回退旧 Rust
  技能/撤退三触控、Hover、固定长按等时序，也禁止静默重试。
- `Output` 和 `Finish` 都是零输入；旧 `after_last_action` 只兼容解析，不改变收尾行为。
- `initial_gap_ms` 继续只服务直接脉冲，不参与 AFA 委托动作。

### 2. ConfirmingAction 的确认窗口

确认屏障位于 `repl-app::Runner`，不修改 `repl-core::Machine` 的命令接口；Runner 已有尺子
引用和 `await_frame()`，由它阻塞等待确认样本。

每次动作派发前记录当前最新样本的 `min_frame_id`（至少取尺子最新值和 Machine 游标的较大
者）。确认只接受 `frame_id > min_frame_id` 的样本；同帧的下一动作必须重新记录水位，绝不
复用上一动作刚确认的那条样本。

样本判定固定如下。这里的 `is_running` 指尺子战斗状态归一化后的运行标志；不得把
`Snapshot::is_running`（费用条识别管线是否在轮询）误当成战斗运行态：

| 尺子样本 | 处理 |
| --- | --- |
| 任意 `is_running=true`（包括 `1x_running`、`0.2x_running` 等） | 立即失败；不等待、不补发 Pause。帧可能已经越过目标帧。 |
| 新的 `paused` 样本但为 `0.2x_paused` | 允许作为过渡样本等待；不能把它标记为动作完成。 |
| 新的、可信的 `1x_paused` 且 `elapsed == target_frame` | 确认动作完成。 |
| `paused` 但倍速为 `2x`、目标帧不一致、已离开战斗 | 失败；不得猜测或重试。 |
| `frame_id <= min_frame_id` | 旧样本，忽略。 |
| 暂停状态未知或尺子样本不可信 | 等待有界超时；超时即失败。 |

因此“短暂运行”不在 **AFA 委托动作的确认窗口** 内；窗口只容许暂停态，且最终必须回到
目标帧的可信 `1x_paused`。逐帧 `pulse()` 自己的脉冲收尾仍可保留 `1x_running → 1x_paused`
的正常过渡判定，两者不能混用。确认超时、尺子读取错误或取消都走统一失败路径。

### 3. AFA 适配器与 INI

- AFA controller 的配置构造路径（当前解析函数为 `AfaBindings::from_ini()`；如职责归并则为
  `AfaController::from_ini()`）内部集中定义四个协议键名：`PressPause`、`ReleasePause`、
  `PauseSkill`、`PauseRetreat`。源码已确认这些名称；运行时仍必须读取并记录用户 INI 中的
  实际值，不能把源码默认值当成用户配置，也不能静默回退。
- 首次实机启动/预检必须日志打印实际读取到的四个键值、配置路径、AFA 进程和权限；若键缺失、
  无法解析或与实机配置不符，预检失败并停止，不修改 INI。
- AFA 实机配置可能是带 BOM 的 UTF-16 LE；读取器按 BOM 接受 UTF-8、UTF-16 LE/BE，仍只读，
  不要求用户转换文件编码。
- `PressPause` 和 `ReleasePause` 是两个不同热键的两次独立 `key_tap`，不是同一按键的
  down/up；`AfaAction` 文档必须保持这一语义。
- 每次委托前重新检查 AFA 进程身份、管理员权限、配置指纹和游戏前台；前台检查失败直接
  中止，不调用 `SetForegroundWindow` 后偷偷重发热键。

### 4. 输入实现边界

- 键盘热键走 `SendInput` 键盘路径。
- `XButton1/2` 走 `MOUSEEVENTF_XDOWN/XUP`；滚轮走 `MOUSEEVENTF_WHEEL/HWHEEL`，实现位于
  `repl-input::mouse`，不能伪装成键盘事件。
- `PauseController::pulse()` 保留给逐帧脉冲；直接暂停入口已明确命名为
  `diagnostic_pause()`/`diagnostic_resume()`，仅供 `step-test` 等诊断工具使用，主运行路径不得
  调用它们执行普通暂停/恢复。

## 里程碑与进度

- [x] M0：读取项目规则、现有计划、状态、代码和 Git；冻结 AFA 委托边界及严格确认语义。
- [x] M1：补齐 XButton、上下/左右滚轮 `SendInput` 路径及局部单元测试。
- [x] M2：实现只读 AFA INI 解析、协议键名集中定义、进程/权限/指纹预检、热键分发和状态灯。
- [x] M3：Session 将普通暂停/恢复、Skill、Retreat 路由到 AFA；Deploy/Pulse 保持 Rust 直注入。
- [x] M3b：开局暂停改由 AFA `AutoBeginPause=1` 负责；Runner 检测开局后零输入等待尺子确认。
- [x] M4：Runner 实现每动作 `min_frame_id` 水位、运行态立即拒绝、`0.2x_paused` 过渡等待、
  目标帧 `1x_paused` 确认、同帧动作隔离及零输入 Finish；PauseController 直接暂停入口收紧为
  诊断命名，并用 `FakeFrameSource` 覆盖确认循环的新样本/运行态路径。
- [x] M5：补齐文档：ACCEPTANCE 删除旧 950ms/Hover/E5-4/E5-7 过期描述，加入 AFA 启动、
  配置核对、状态灯、前台失败和新确认语义；同步第三方声明、CURRENT_STATE 和本计划。
- [x] M6：补齐 AFA `AfaBindings`/INI 边界测试（缺键、重复键、键盘/XButton/滚轮、日志可观测性）。
- [x] M7：运行全工作区自动验证：
  `cargo test --workspace`、`cargo clippy --workspace --all-targets -- -D warnings`、
  `cargo build --release`，并检查 `cargo fmt --all`、`git diff --check`。
- [ ] M8：用户实机验收（进行中、未稳定通过）。当前构建连续 4 次完整确认后，第 5 次因开局
  尺子盲区从第 0 帧跳到第 33 帧并越过首目标 10；更早还有运行期 Pause 临界失败和一次错帧。
  外部程序只由用户操作。
- [ ] M9：M8 通过后才做录像 CSV 的 10/10 目标帧核验；未通过不得宣称端到端完成。

## 实机验收入口

1. 用户启动管理员权限的 AFA、ArknightsCostBarRuler 和《明日方舟》PC 客户端。
2. 首次预检核对当前 AFA 配置（本项目验收 profile 的预期值；不是 AFA 源码默认值）：
   `PressPause=g`、`ReleasePause=Space`、`PauseSkill=XButton2`、`PauseRetreat=XButton1`、
   `AutoBeginPause=1`、`DefaultStrongHoldProtocol=0`。若本 profile 的实际值不同，先在 AFA
   界面完成配置并重新预检；复刻器不修改 INI。若其他实际值不同，先记录实际
   值并停止本轮验收；只有协议键名/格式与源码不一致时才调整适配器集中常量/解析，不改 AFA 配置。
3. 先验证普通暂停/恢复；再分别以 Skill、Retreat 作为最后动作，确认画面和尺子都停在目标
   帧的 `1x_paused`，且 Finish 没有补输入。
4. 通过后再跑 Deploy → Skill → Retreat 的同帧/跨帧组合和 §5.4 的录像核验。
5. 任一动作前台检查失败、AFA 失联、运行态样本出现或目标帧越过，立即记录日志并中止；
   不盲重试热键。

## 已验证证据

- `cargo test -p repl-input`：34 个测试通过，包含 AFA UTF-16 LE/BE INI 回归覆盖。
- `cargo test -p repl-app`：20 个测试通过。
- `cargo check -p repl-app`：通过。
- `cargo test --workspace`：通过（各 crate 全部测试和 doctest 通过；`repl-input` 当前 34 项，
  `repl-core` 当前 100 项，`repl-app` 当前 20 项）。
- `cargo clippy --workspace --all-targets -- -D warnings`：通过。
- `cargo build --release`：通过。
- `cargo fmt --all`、`git diff --check`：通过。
- 当前 20:09 构建已有 AFA/游戏实机证据：连续 4 次在 10/60/180 帧确认并零输入结束；该证据
  证明链路可工作，不证明稳定通过，因为紧接着一次在首动作前越过到第 33 帧。

## 风险、发现与结果记录

- AFA 热键没有回执；尺子确认只能证明派发后停在目标帧，技能/撤退的画面效果仍需用户录像确认。
- 2026-07-27 实机首次跑到第 10 帧部署后，运行期 AFA Pause 未在 4 条新样本内确认，状态机以
  `PauseIneffective` 中止；开局所有权调整不等于该运行期问题已解决，仍需单独修复和复验。
- 2026-07-27 当前构建连续 4 次完整完成 10/60/180 帧后，第 5 次先收到开局
  `not_in_battle`/不可信样本，恢复后的第一条可信读数直接到第 33 帧；首目标 10 已越过且没有
  派发任何动作。这是巡航前的尺子盲区，不是 `initial_gap_ms` 或逐帧脉冲问题。
- 工作区父目录及 `target\release` 下都存在较早的二进制/日志副本，历史日志仍记录旧收尾或 Hover
  时序；M8 只能使用当前 20:09 后构建的 `target\release\repl-app.exe` 重新生成的日志作为证据。
- 当前用户 `%APPDATA%\ArknightsFrameAssistant\PC\Settings.ini` 已只读核对为验收 profile，
  包含 `AutoBeginPause=1`、`DefaultStrongHoldProtocol=0`；复刻器不会代改配置。
- 2026-07-27 首次 M8 启动确认该文件为 UTF-16 LE（BOM `FF FE`），暴露 UTF-8-only 读取缺陷；
  已改为 BOM 感知解码并完成全工作区测试、Clippy 和 release 重建，等待用户复验 AFA 状态灯。
- `Settings.ini` 的四个键名已由同工作区 AFA 源码确认，但实际值、热键捕获和动作效果仍必须由首次
  实机日志校验；发现协议格式不一致时先停用委托，不做兼容性猜测。
- 游戏失焦、AFA 重启、配置改动和非 1x 状态都视为不可恢复的本次运行失败。
- 每个里程碑完成后更新 `docs/CURRENT_STATE.md`；长期边界变化才新增 ADR，不把聊天记录当状态。
