# 当前状态

更新时间：2026-07-29 17:09:19 +08:00

## 当前目标

人工打轴编辑器、风险确认式“继续”和 AFA 委托运行链路均已实现。M8 启动接管与
Deploy/Skill/Retreat 运行链已由用户完成实机验收；当前阶段进行本地 `develop` 知识收尾和提交，
不上传远端。后续只剩相互独立的继续功能实机验收、编辑器 GUI 验收和 M9 精确帧核验。

## 现役实现与已验证成果

- AFA 负责自动开局暂停、普通暂停/恢复、暂停技能和暂停撤退；Rust 负责部署拖拽、自适应逐帧
  脉冲、鼠标定位/停靠和单次 AFA 热键触发。AFA 配置只读，失败时不回退旧时序。
- 第 0 帧“开始复刻”执行两次人工焦点交接、编队绑定和新尺子样本接管；Runner 不发送开局暂停，
  也不自动抢游戏焦点。动作前持续复核尺子，动作后只接受新水位上的目标帧可信暂停样本。
- Pulse 收尾、运行期 Pause 帧宽限和 Resume running 回执均有状态机回归覆盖；同帧动作分别使用
  新样本水位，最终 Finish 为零输入。
- 2026-07-29，用户明确确认当前验收二进制的 M8 §3–§4 全部通过：开始门禁、手动焦点交接、
  已有档案恢复、paused 人工绑定、running 无档案安全中止、Deploy/Skill/Retreat 全流程、热键无
  重复派发、零跨帧以及最终目标帧 `1x_paused` 零输入收尾。权威记录见
  [当前版本实机验收](REAL_MACHINE_ACCEPTANCE.md)；截图和运行日志未纳入 Git。
- 非零可信暂停帧可冻结“继续截止帧”；前缀动作标为“用户确认完成”，新会话按作业播种场上状态，
  最终接管不得越过下一动作帧。跨编队头像档案支持后续再部署；继续结果不能计入 M9。
- 人工作业编辑器提供保留未知 JSON 字段的 `JobDocument`、严格 `Copilot` 编译边界、分轨时间轴、
  同帧排序、撤销/重做、原子保存、编队维护和逻辑地图选点。编辑模式不探测 AFA、不创建
  `Session` / `Runner`、不定位游戏窗口且不产生输入。
- 保存/另存为后的 GUI 闪退已由回归测试覆盖：保存回调在刷新 Slint 前释放 `RefCell` 可变借用。
- 自动门禁通过：`cargo test --workspace`、`cargo clippy --workspace --all-targets -- -D warnings`、
  `cargo build --release`、`cargo fmt --all -- --check` 和 `git diff --check`。
- 长期边界与时序决定位于 `docs/decisions/0001`–`0009`；领域术语以 `CONTEXT.md` 为准。

## 版本与发布状态

- 分支：本地 `develop`。本轮收尾提交只保存在本地；没有 push、PR 或其他远端写入。
- `origin/develop` 的本地跟踪引用仍为 `d85eb53`；本次没有联网刷新该引用。
- 验收二进制：`target\release\repl-app.exe`，大小 `14,710,272` 字节，SHA-256
  `4D510B3146FA40C95437E3E2914E7E8E42A2C91C53CFA8A8E035EEFF267AF8BD`。
- 状态：代码 implemented、自动门禁 locally verified、M8 real-machine verified、knowledge closed；
  尚未 pushed、PR opened、merged、deployed，M9 也未完成。

## 未完成 / 待验证

- 风险确认式“继续”：按[当前版本实机验收 §5](REAL_MACHINE_ACCEPTANCE.md)核对 F11、F1、冻结边界、
  最后动作后禁用和缺头像档案门禁。自动测试不能证明用户确认的前缀动作与真实画面一致。
- 编辑器 GUI：按[编辑器实机验收](EDITOR_ACCEPTANCE.md)核对文件对话框、地图点选、长文本输入、
  窗口缩放和严格零输入边界。
- M9：只能使用第 0 帧普通精确运行完成录像和 CSV 10/10；任何“带风险完成”样本必须排除。

## 准确下一步

先任选“继续功能 §5”或编辑器 GUI 做人工验收；两者不阻塞已完成的 M8。准备证明精确帧稳定性时，
再按[当前版本实机验收 §6](REAL_MACHINE_ACCEPTANCE.md)执行 M9 10/10。

## 风险与工作区说明

- AFA 热键没有动作效果回执；M8 的技能/撤退画面效果依据用户实机验收，M9 仍须录像逐帧证明。
- 继续功能完全相信作业和用户对动作前缀的确认；作业与真实场上状态不一致可能导致后续点击错误。
- 第 0 帧暂停可能冻结费用条渐显；正式精确验收作业首动作至少第 60 帧。早帧样例只用于回归。
- 根目录 `deploy-scan.png`、`tile-preview-1_7.png` 和 `target/` 是被 `.gitignore` 排除的本地验收/
  构建产物，未纳入提交，也未在本次收尾中删除。
