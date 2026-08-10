# 明日方舟帧级操作复刻器

**Arknights Operation Replicator** 是面向《明日方舟》PC 客户端的帧级作业编辑与复刻工具。

当前版本：**0.2.0**。

它使用 [ArknightsCostBarRuler](https://github.com/ZeroAd-06/ArknightsCostBarRuler) 提供的绝对逻辑帧
作为唯一时间基准，并通过 AFA（Arknights Frame Assistant）完成暂停、恢复、逐帧、技能和撤退。
与按费用、击杀数或技能状态推进的普通自动战斗不同，本项目只在作业指定的**绝对帧**执行动作。

```text
人工编辑帧作业
    ↓
可选：战前扫描编队并人工确认
    ↓
进入关卡，等待 AFA 自动暂停
    ↓
复刻器接管尺子帧并准备编队绑定
    ↓
运行 → 提前暂停 → 逐帧到目标 → 执行动作 → 尺子确认
    ↓
最后动作完成，停留在目标帧
```

> 当前实现、构建身份和待验项目以 [当前状态](docs/CURRENT_STATE.md) 为准。首次安装请从
> [完整验收手册](ACCEPTANCE.md) 开始；开发构建按
> [当前版本实机验收](docs/REAL_MACHINE_ACCEPTANCE.md) 验证。

## 功能

### 帧级复刻

- 支持 `Deploy`、`Skill`、`Retreat`、`Output` 动作。
- 动作按绝对逻辑帧调度；同帧动作按作业顺序依次执行。
- 每次输入前后都用新的尺子样本复核帧号、倍速、暂停状态和战斗状态。
- 最后动作和 `Finish` 均为零输入收尾，画面保持在最终目标帧。
- 非第 0 帧可使用风险确认式“继续”，从用户确认完成的动作前缀之后接着执行。

### 人工作业编辑器

- 分轨时间轴、地图格子点选、同帧排序、复制、删除、撤销和重做。
- 文本与数字在回车或失焦时一次提交，避免每个字符形成一条撤销记录。
- 编队干员支持按名称片段搜索本地 MAA 干员目录。
- 干员和部署方向使用下拉菜单；方向包含上、下、左、右和无方向。
- 支持 `Skill + location` 的地图装置技能。
- 尺子连接后以轻量 33ms 刷新显示当前绝对帧；“跟随尺子”只控制时间轴光标。
- 编辑模式严格零输入：不调用 AFA，不定位游戏窗口，不向游戏发送鼠标或键盘操作。

完整操作步骤以及主线、活动、危机合约、集成战略、生息演算、保全派驻等地图的关卡标识格式，见
[作业编辑器使用说明](docs/JOB_EDITOR_GUIDE.md)。

### 编队、召唤物与装置

- 开局可见卡片可人工绑定，也可从已保存的头像档案恢复。
- 战前“扫描编队”兼容 MAA 的 PaddleOCR 模型、字典和 `battle_data.json`；扫描结果必须由用户确认。
- 首次出现的召唤物在目标帧延迟绑定，之后可在同局、同编队和其他编队复用全局头像档案。
- 地图预置装置按逻辑格子触发技能，不进入部署栏绑定，也不写入头像档案。
- OCR、头像桥接或历史档案存在歧义时回退人工确认，不自动猜测。

## 当前验收状态

| 范围 | 状态 |
| --- | --- |
| 帧源、地图投影、部署栏基础识别 | 已完成实机验收 |
| AFA 暂停/恢复/逐帧与 Deploy → Skill → Retreat | M8 实机通过 |
| AFA 单帧推进录像与 CSV | M9 10/10 通过 |
| 编辑器、风险继续、延迟绑定、战前 OCR/F0 桥接 | 自动门禁通过，仍需按开发构建手册实机验收 |
| 地图装置技能、编队目录搜索、部署方向边界 | 自动门禁通过，仍需实机验收 |
| 部署后 Resume 稳定屏障、编辑页实时绝对帧 | 自动门禁通过，仍需实机验收 |

M8/M9 是已经完成的历史验收基线，不能替代后来新增功能的实机验收。风险确认式“继续”也不能计入
精确复刻的 M9 结果。

## 运行要求

1. Windows 10/11。
2. 《明日方舟》PC 客户端 `Arknights.exe`。
3. AFA 已启动并配置：
   - `AutoBeginPause=1`；
   - `PressPause`、`ReleasePause`、`PauseSkill`、`PauseRetreat` 和 `33ms` 热键有效；
   - AFA、尺子和本程序与游戏使用一致的完整性级别。
4. ArknightsCostBarRuler 已运行并完成费用条校准；复刻器通过
   `ws://127.0.0.1:2606` 读取帧状态。
5. 可用的 MAA `resource/` 目录，用于地图投影、部署栏识别、干员目录与可选战前 OCR。
   可在“作业编辑”页点击“选择…”指定，也可设置 `REPLICATOR_MAA_RESOURCE`。

本工具依赖费用尺，费用条无法可靠工作的关卡或状态也不适合作为复刻环境，例如费用已满、费用回复
被锁定和剿灭作战。

## 构建与启动

项目使用 Rust 2021 workspace。在 PowerShell 中执行：

```powershell
cargo build --release
.\target\release\repl-app.exe
```

首次启动后：

1. 在“作业编辑”页选择 MAA 资源目录并检查尺子连接。
2. 按 [完整验收手册](ACCEPTANCE.md) 验证帧源、单帧推进、地图投影和部署栏识别。
3. 打开或创建作业，在编辑页完成打轴。
4. 切换到“复刻执行”，按界面提示完成战前扫描、进关、焦点交接和绑定。

## 编辑作业

完整说明见 [作业编辑器使用说明](docs/JOB_EDITOR_GUIDE.md)。其中列出了当前 MAA Tile Pos 索引覆盖的
全部地图家族、推荐 `stageId` 格式、特殊模式限制以及查询准确关卡标识的 PowerShell 命令。

1. 填写标题和 MAA 关卡标识，例如 `main_01-07`。
2. 在“编队干员”中输入名称片段，从本地 MAA 目录选择干员；自定义名称仍可手动添加。
3. 将时间轴光标移动到目标绝对帧，添加部署、技能/装置、撤退或注释动作。
4. 在属性面板选择目标、逻辑格子和部署方向。
5. 处理底部诊断。无效草稿允许保存，但不能进入复刻运行。
6. 保存后切换到“复刻执行”。

编辑器保存 MAA Copilot schema 的超集，并尽量保留当前版本不认识的顶层、文档、编队和动作字段。
第一期采用人工打轴，不监听或自动识别真实鼠标操作。

### 地图装置技能

地图装置使用 `Skill + location`。名称可选，显式格子优先于名称：

```json
{
  "type": "Skill",
  "frame": 240,
  "name": "留声机",
  "location": [5, 3]
}
```

普通干员技能可以省略 `location`，运行时按之前记录的部署位置定位。

## 运行作业

### 从第 0 帧开始

1. 可选：在战前编队确认界面点击“扫描编队”，核对并确认 OCR 结果。
2. 进入关卡，等待 AFA 自动暂停。
3. 确认尺子处于可信的战斗内第 0 帧 `1x_paused`，点击“开始复刻”。
4. 按提示手动点击游戏窗口。程序不会自动抢焦点，也不会发送开局暂停。
5. 完成可见卡片绑定；准备结束后再次按提示切回游戏。
6. 程序取得新的可信尺子样本后开始执行。

部署动作完成后，程序会等待同目标帧稳定暂停窗口，再发送一次 Resume；任何运行态、错帧、非 1 倍速
或离开战斗的样本都会中止当前运行。

### 从非零帧继续

当尺子停在非零可信 `1x_paused` 帧且作业还有后续动作时，“继续”按钮可用：

- 当前帧被冻结为截止帧，所有 `frame <= 截止帧` 的动作视为用户确认完成。
- 弹窗展示动作前缀、推导出的场上状态、下一动作和头像档案缺口。
- 确认前再次检查尺子和 AFA；如果已经越过下一动作帧，本轮立即终止。
- 后续动作在全新会话中执行，结果标记为“带风险完成”。

例如作业在 F10 部署、F40 开技能，游戏停在 F11 时，可确认 F10 已由用户完成，然后从 F40 继续。

## 作业格式

作业是 MAA Copilot schema 的超集：每个可执行动作增加 `frame`，顶层增加 `frame_replicator`。
完整示例见 [examples/sample-job.json](examples/sample-job.json)。

```json
{
  "stage_name": "main_01-07",
  "frame_replicator": {
    "version": 1,
    "speed": "1x",
    "after_last_action": "pause"
  },
  "opers": [
    { "name": "桃金娘", "skill": 1 }
  ],
  "actions": [
    {
      "type": "Deploy",
      "frame": 10,
      "name": "桃金娘",
      "location": [3, 2],
      "direction": "Right"
    },
    {
      "type": "Skill",
      "frame": 40,
      "name": "桃金娘"
    },
    {
      "type": "Retreat",
      "frame": 60,
      "name": "桃金娘"
    }
  ]
}
```

- `frame` 是从战斗开始累计的绝对逻辑帧；1 倍速下 30 帧约等于 1 秒。
- 帧号必须非递减；相同帧号表示依次执行同帧动作。
- `kills`、`costs`、`cost_changes`、`cooling`、`pre_delay` 和 `post_delay` 可以保留，但帧模式不把
  它们作为等待条件。
- `opers` 表示开局编队。首次由 Deploy 引入的召唤物或装置可放在
  `frame_replicator.deferred_targets`，不必伪装成开局干员。

## 诊断工具

Release 构建会在 `target/release/` 生成：

| 程序 | 用途 |
| --- | --- |
| `repl-app.exe` | 主程序 |
| `frames-probe.exe` | 打印尺子的帧号与状态 |
| `afa-step-test.exe` | 通过 AFA `33ms` 热键执行最多 500 次单帧统计 |
| `formation-scan.exe` | 战前编队 OCR 只读诊断 |
| `deploy-scan.exe` | 部署栏识别诊断 |
| `tile-preview.exe` | 在截图上绘制地图格子投影 |
| `step-test.exe` | 旧 Rust 直接脉冲诊断，不属于主复刻路径 |
| `step-sweep.exe` | 旧脉冲传递函数扫描 |

运行日志写入 `repl-app.exe` 同目录的 `replicator.log`。

## 工程结构

```text
crates/
├─ repl-core/    作业模型、地图投影、部署手势和复刻状态机
├─ repl-frames/  尺子 WebSocket 客户端
├─ repl-input/   AFA 热键、触控与鼠标键盘适配
├─ repl-vision/  部署栏识别、MAA 兼容 OCR 与头像桥接
├─ repl-capture/ 游戏窗口定位与 Windows Graphics Capture
└─ repl-app/     Slint 图形界面和运行编排
```

设计边界见 [领域上下文](CONTEXT.md) 和 [架构决策](docs/decisions/)。

## 开发验证

提交改动前运行：

```powershell
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release
cargo fmt --all -- --check
git diff --check
```

GUI、游戏输入、焦点交接和真实 OCR 仍需按实机验收文档人工确认；自动测试通过不等于实机通过。

## 许可证与第三方代码

本项目以 [AGPL-3.0-only](LICENSE) 发布。

项目包含从 MaaAssistantArknights（AGPL-3.0-only）和 arknights-frame-assistant
（GPL-3.0-only）移植并改写的实现。分发程序时必须同时满足相应 copyleft 义务。逐文件来源和
ArknightsCostBarRuler（MIT）说明见 [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md)；Windows
Release 的锁定 Rust 运行依赖、版本和许可证表达式见
[THIRD-PARTY-LICENSES.md](THIRD-PARTY-LICENSES.md)。

## 免责声明

《明日方舟》著作权归上海鹰角网络科技有限公司所有，本项目与其没有隶属或授权关系。

本项目不注入游戏进程，不读写游戏内存，不修改游戏文件，也不与游戏服务器通信；它只使用 Windows
Graphics Capture、`InjectTouchInput` 和 `SendInput` 等操作系统接口。使用者需自行承担使用自动化工具
可能带来的账号和游戏协议风险。
