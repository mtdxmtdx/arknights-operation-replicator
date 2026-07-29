# 明日方舟帧级操作复刻器 (Arknights Operation Replicator)

把一份"作业"（动作序列）以**逐帧精确**的方式在《明日方舟》PC 客户端上复刻出来。

与 MAA 的自动战斗相比，唯一但决定性的区别：**每一步的等待条件不是视觉事件
（击杀数 / 费用 / 技能就绪），而是绝对逻辑帧。**

```
用户进关并由 AFA 暂停 → 点击开始 → 手动切回游戏焦点 → 编队准备 → 接管尺子帧 → 正常运行 → 提前暂停 → 逐帧推进 → 注入动作 → 下一动作
```

## 状态

核心 AFA 委托与运行链路已于 2026-07-29 完成 M8 实机验收；M9 的录像与 CSV 10/10 精确帧核验
仍未执行。首次安装按[完整验收手册](ACCEPTANCE.md)执行；当前构建的验收状态以
[当前版本实机验收](docs/REAL_MACHINE_ACCEPTANCE.md) 为唯一入口。技能和撤退由 AFA 执行，
复刻器只在派发后的新尺子样本确认目标帧与暂停状态后才把动作标记为完成。

| 验收节 | 状态 |
| --- | --- |
| 1 帧源接入 | ✅ 实机通过（2026-07-26） |
| 2 单帧脉冲精度 | ✅ 实机通过（100 次脉冲 0 跨帧、35% 命中，纯自旋定时修复后） |
| 3 地图坐标投影 | ✅ 实机通过（1-7，绿十字对齐格子中心，人工核对） |
| 4 部署栏识别 | ✅ 实机通过（2560×1440，`uiScaler=0`，职业判定已人工核对） |
| 5 端到端复刻 | ✅ M8 实机通过（2026-07-29）；M9 录像与 CSV 10/10 待执行 |
| 6 资源占用 | ✅ 当前二进制 14.0 MiB；历史空闲私有工作集 5.4MB |

输入适配、AFA 解析和动作确认测试已通过；`cargo test --workspace`、clippy、release 构建、
格式检查和 diff 检查均已通过。M8 实机已确认启动门禁、焦点交接、绑定路径、AFA
Resume/Pause 委托、Deploy → Skill → Retreat、零跨帧和最终目标帧暂停。M9 尚未执行；带风险的
“继续”运行不能计入 M9。

## 运行前提

1. **Windows 10/11**，《明日方舟》**PC 客户端**（`Arknights.exe`）。
2. **AFA（Arknights Frame Assistant）** 已由用户以管理员权限启动；复刻器只读
   `%APPDATA%\ArknightsFrameAssistant\PC\Settings.ini`，要求 `PressPause`、
   `ReleasePause`、`PauseSkill`、`PauseRetreat` 四项热键有效，且开启 AFA 自动开局暂停、
   关闭卫戍协议默认模式。AFA 不可用时复刻器不会回退旧 Rust 时序。
3. **[ArknightsCostBarRuler](https://github.com/ZeroAd-06/ArknightsCostBarRuler)** 独立运行，
   并已完成一次费用条校准。它是帧数的唯一真源，复刻器通过其 `127.0.0.1:2606`
   本地 API 读取绝对帧。
4. 一份 **MAA 资源目录**（`resource/`），用于地图格子投影和部署栏识别。可以是已安装的
   [MaaAssistantArknights](https://github.com/MaaAssistantArknights/MaaAssistantArknights)，
   也可以单独获取其资源仓库。**本仓库不分发这些游戏资源文件**，首次运行向导会让你指定路径。

尺子无法工作的场景同样是本工具无法工作的场景：**部署费用已满、费用回复被锁定、剿灭作战**。

## 构建与运行

```powershell
cargo build --release
```

产物在 `target/release/`：

| 程序 | 用途 |
| --- | --- |
| `repl-app.exe` | 主程序（图形界面） |
| `frames-probe.exe` | 实时打印尺子的帧数与状态，用于验收帧源 |
| `step-test.exe` | 单帧脉冲精度实测，**最关键的验收工具** |
| `step-sweep.exe` | 脉冲传递函数扫描，`step-test` 不过时用它定位原因 |
| `tile-preview.exe` | 把算出的格子坐标画到真实截图上，验证地图投影 |
| `deploy-scan.exe` | 部署栏识别自检 |

第一次用请先走一遍 [完整验收手册](ACCEPTANCE.md)；验证当前开发构建时再执行
[M8/M9 实机验收](docs/REAL_MACHINE_ACCEPTANCE.md)。

## 人工作业编辑器

主程序默认打开“作业编辑”页。该页是严格零输入模式：不探测 AFA、不创建复刻会话，也不会向
游戏发送鼠标或键盘操作。可以在没有游戏的情况下离线编辑；尺子在线时可开启“跟随尺子”，把
时间轴光标吸附到当前绝对帧。

1. 填写作业标题和关卡标识，在编队栏加入干员。
2. 把光标设到目标帧，添加部署、技能、撤退或注释动作。
3. 在右侧属性面板填写干员、部署格与朝向；找到 MAA 地图资源后可直接点逻辑格子。
4. 处理底部错误诊断。无效草稿仍可保存，但不能交给复刻模式运行。
5. 保存后切到“复刻执行”。通过严格校验的文档会沿用原有 AFA/尺子复刻链路。

编辑器保存 MAA copilot 超集 JSON，打开并修改旧作业时会保留当前版本不认识的顶层、文档、编队
和动作字段。时间轴支持撤销/重做、复制/删除动作和同帧顺序调整。第一期只做人工打轴，不监听或
自动识别真实鼠标操作。

## 作业格式

MAA copilot schema 的超集：现成 MAA 作业加上每个动作的 `frame`（绝对逻辑帧）
和一个顶层 `frame_replicator` 块即可。示例见
[examples/sample-job.json](examples/sample-job.json)。该文件当前使用 `10/40/60` 帧来回归早帧
Resume/Pause/Pulse 时序；正式端到端验收应先复制并把首动作移到至少第 60 帧，以避开尚未根治的
开局费用条盲区。

## 工程结构

```
crates/
├─ repl-core/    纯逻辑：作业模型、地图投影、坐标映射、部署手势、帧复刻状态机
├─ repl-frames/  尺子的 WebSocket 客户端
├─ repl-input/   触控注入（InjectTouchInput）、键盘、游戏内键位、逐帧脉冲
├─ repl-vision/  部署栏识别（纯 Rust 模板匹配，不依赖 OpenCV）
├─ repl-capture/ 窗口定位 + Windows Graphics Capture 按需截图
└─ repl-app/     Slint 界面、配置、编排
```

## 许可证

**AGPL-3.0-only**（见 [LICENSE](LICENSE)）。

这不是随意选择的。本项目移植了 **MaaAssistantArknights（AGPL-3.0-only）** 与
**arknights-frame-assistant（GPL-3.0-only）** 的代码，把源代码翻译成 Rust 属于制作衍生
作品，copyleft 义务照常生效；GPLv3 §13 允许两者合并，合并结果必须整体受 AGPLv3 约束。

因此：分发本程序（含二进制）必须同时提供完整对应源代码并以 AGPL-3.0-only 授权；
若改造成网络服务，AGPL §13 要求向使用者提供源代码；**不能闭源、不能专有再授权**。

逐文件的来源对照、以及对 ArknightsCostBarRuler（MIT）的说明，见
[THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md)。

## 免责声明

《明日方舟》著作权归上海鹰角网络科技有限公司所有，本项目与其无任何隶属或授权关系。

本项目**不注入游戏进程、不读写游戏内存、不修改游戏文件、不与游戏服务器通信**，
只使用操作系统的公开接口（Windows Graphics Capture 截图、`InjectTouchInput` /
`SendInput` 输入注入）。即便如此，使用自动化工具仍可能违反游戏用户协议，风险自负。

## 当前启动流程

1. 用户先进入关卡，等待 AFA 自动暂停。
2. 确认尺子显示带 `frame_id` 的可信战斗内 `1x_paused` 第 0 帧，点击“开始复刻”。
3. 复刻器保持显示，等待用户手动点击游戏窗口；不会自动抢游戏焦点。
4. 焦点后的新尺子样本决定绑定模式：paused 可人工绑定，running 只允许已有档案自动恢复。
5. 编队准备完成后再次等待用户把游戏切回前台，以最终焦点后的新可信样本初始化 Machine。

## 非零帧继续

可信战斗内 `1x_paused` 停在非零帧、且作业仍有更晚动作时，“继续”按钮可用。点击后先显示风险
确认，不会立即产生输入：

- 当前帧被冻结为截止帧，所有 `frame <= 截止帧` 的动作视为“用户确认完成”；同帧动作不会拆开。
- 弹窗列出这些动作、按作业推导的场上干员与位置、下一动作、头像档案缺口和风险。
- 确认时再次检查 AFA 与尺子。画面若已冲过下一动作帧，本轮直接拒绝，不会多跳动作。
- 继续使用全新会话执行后续动作。结果标记为“带风险完成”，不能计入精确复刻 M9 验收。

例如 `examples/test1.json` 停在 F11：确认后 F10 桃金娘部署标为“用户确认完成”，场上位置按作业
推导为 `(3,2)`，只需绑定后续要部署的风笛，然后从 F40 桃金娘技能继续。F1 也可用来明确承担
冲过第 0 帧的风险，此时没有动作被跳过。
