# 明日方舟帧级操作复刻器 (Arknights Operation Replicator)

把一份"作业"（动作序列）以**逐帧精确**的方式在《明日方舟》PC 客户端上复刻出来。

与 MAA 的自动战斗相比，唯一但决定性的区别：**每一步的等待条件不是视觉事件
（击杀数 / 费用 / 技能就绪），而是绝对逻辑帧。**

```
等待开局 → 第零帧暂停 → 编队匹配 → 正常运行 → 提前暂停 → 逐帧推进 → 到目标帧暂停 → 注入动作 → 下一动作
```

## 状态

核心流程已实现，但本轮 AFA 委托重构尚未完成端到端实机验收 —— 按 [ACCEPTANCE.md](ACCEPTANCE.md)
逐节执行。技能和撤退现在由 AFA 执行，复刻器只在派发后的新尺子样本确认目标帧与暂停状态后
才把动作标记为完成。

| 验收节 | 状态 |
| --- | --- |
| 1 帧源接入 | ✅ 实机通过（2026-07-26） |
| 2 单帧脉冲精度 | ✅ 实机通过（100 次脉冲 0 跨帧、35% 命中，纯自旋定时修复后） |
| 3 地图坐标投影 | ✅ 实机通过（1-7，绿十字对齐格子中心，人工核对） |
| 4 部署栏识别 | ✅ 实机通过（2560×1440，`uiScaler=0`，职业判定已人工核对） |
| 5 端到端复刻 | ⚠️ 实机部分通过但未稳定：连续 4 次完成，随后 1 次开局尺子盲区导致首目标越过 |
| 6 资源占用 | ✅ 12.9MB 二进制，空闲私有工作集 5.4MB |

输入适配、AFA 解析和动作确认测试已通过；`cargo test --workspace`、clippy、release 构建、
格式检查和 diff 检查均已通过。实机已证明 AFA 能完成 Deploy → Skill → Retreat 并在目标帧
暂停，但首目标过早时仍可能因开局费用条不可读而越过，尚不能开始录像 10/10 帧号核验。

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

第一次用请先走一遍 [ACCEPTANCE.md](ACCEPTANCE.md)。

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
