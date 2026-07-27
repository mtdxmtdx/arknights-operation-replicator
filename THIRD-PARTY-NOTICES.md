# 第三方来源与许可证声明

本文件说明本项目（Arknights Operation Replicator，下称"复刻器"）复用了哪些上游项目、
以什么方式复用、以及由此产生的许可证义务。

## 结论先行

**本项目整体以 `AGPL-3.0-only` 发布。** 这不是一个可选项，而是上游许可证的强制结果：

| 上游项目 | 许可证 | 复用方式 | 是否产生 copyleft 义务 |
| --- | --- | --- | --- |
| [MaaAssistantArknights](https://github.com/MaaAssistantArknights/MaaAssistantArknights) | **AGPL-3.0-only** | **移植其源代码算法**（见下） | **是** |
| [arknights-frame-assistant](https://github.com/CloudTracey/arknights-frame-assistant) | **GPL-3.0-only** | **移植其源代码**（见下） | **是** |
| [ArknightsCostBarRuler](https://github.com/ZeroAd-06/ArknightsCostBarRuler) | MIT | 仅通过其本地 HTTP/WebSocket API 交互，**未复制任何代码** | 否（仅需署名，见下） |

把源代码从 C++/AutoHotkey **翻译成 Rust 属于制作衍生作品**（GPLv3 定义的 "modified version"），
不是"重新实现"，因此 copyleft 义务照常生效。

AGPL-3.0 与 GPL-3.0 的合并依据是 GPLv3 第 13 条：GPLv3 明确允许把 GPLv3 作品与 AGPLv3
作品合并成一个整体，合并结果整体上要满足 AGPLv3 第 13 条（网络交互时提供源码）的要求。
因此合并后的唯一自洽选择就是 AGPL-3.0-only。

### 这对使用者意味着什么

- 分发本程序（含二进制）时必须同时提供**完整对应源代码**，并以 AGPL-3.0-only 授权。
- 如果把本程序改造成**通过网络提供服务**（例如做成远程代打服务端），AGPL 第 13 条要求
  向所有用户提供该修改版的源代码。
- **不能**把本项目闭源，也不能以专有许可证再授权。

## 逐项说明：从 MaaAssistantArknights 移植了什么

上游：`MaaAssistantArknights`，AGPL-3.0-only，Copyright (C) MaaAssistantArknights 开发者。

以下 Rust 代码是对上游 C++ 源文件的翻译/改写，属于衍生作品。每个对应的 Rust 源文件
头部都带有指明来源文件的 SPDX 注释：

| 复刻器中的位置 | 上游来源 |
| --- | --- |
| `repl-core/src/tile.rs` | `3rdparty/include/Arknights-Tile-Pos/TileCalc2.hpp`（相机矩阵 → 屏幕坐标投影） |
| `repl-core/src/level.rs` | `3rdparty/include/Arknights-Tile-Pos/TileDef.hpp`、`src/MaaCore/Config/Miscellaneous/TilePack.cpp` |
| `repl-core/src/copilot.rs` | `src/MaaCore/Config/Miscellaneous/CopilotConfig.cpp`、`src/MaaCore/Common/AsstBattleDef.h` |
| `repl-core/src/gesture.rs` | `src/MaaCore/Task/BattleHelper.cpp` (`deploy_oper`、`fix_swipe_out_of_limit`)、`src/MaaCore/Controller/MinitouchController.cpp`（滑动插值与缓动） |
| `repl-core/src/machine.rs` | `src/MaaCore/Task/Miscellaneous/BattleProcessTask.cpp`（动作序列主循环的结构；每步等待条件由视觉条件改为绝对逻辑帧） |
| `repl-core/src/geom.rs` | `src/MaaCore/Common/AsstTypes.h`（`Point` / `Rect` / `rectMove` 语义） |
| `repl-vision/src/deployment.rs` | `src/MaaCore/Vision/Battle/BattlefieldMatcher.cpp` (`deployment_analyze` 及其子分析)、`src/MaaCore/Task/BattleHelper.cpp` (`analyze_oper_with_cache`) |
| `repl-vision/src/ncc.rs` | 等价于 OpenCV `TM_CCOEFF_NORMED`；算法本身是公共知识，但阈值/掩码参数取自 MAA |
| 各处数值常量 | `resource/tasks/tasks.json`（`BattleOpersFlag`、`BattleOper*`、`BattleSwipeOper`、`BattleUseOper`、`BattlePause` 等条目） |

## 逐项说明：从 arknights-frame-assistant 移植了什么

上游：`arknights-frame-assistant`，GPL-3.0-only，Copyright (C) CloudTracey 及贡献者。

| 复刻器中的位置 | 上游来源 |
| --- | --- |
| `repl-input/src/touch.rs` | `src/lib/touch_injection.ahk`（`InitializeTouchInjection` / `InjectTouchInput` 的 `POINTER_TOUCH_INFO` 填法与 down/update/up 序列） |
| `repl-input/src/game_keys.ahk` 对应的 `repl-input/src/game_keys.rs` | `src/lib/game_keys.ahk`（注册表 `KEYBOARD_SETTING_V*` 读取与解析、Unity keyId → 按键名映射表、默认按键） |
| `repl-input/src/stepper.rs` | `src/lib/hotkey_actions.ahk` (`Action16ms` / `Action33ms` / `Action166ms` 的暂停脉冲时序) |
| `repl-input/src/clock.rs` | `src/lib/hotkey_actions.ahk` (`USleep` 的 QPC 自旋延时) |
| `repl-core/src/lib.rs` 中的 `LOGICAL_FPS_1X` 等常量 | 上游 README 的"关于游戏内帧率"一节 |

当前版本的 `repl-app/src/session.rs` **不再移植**上游的
`ActionPauseSelect` / `ActionPauseSkill` / `ActionPauseRetreat` 的选中与功能键
时序。它只负责部署触控、把 Skill / Retreat 的目标定位到当前鼠标位置，并向
用户已启动的外部 AFA 发送一次配置中的热键；AFA 的技能/撤退时序仍由 AFA 自己执行。
`repl-input/src/afa.rs` 是本项目的只读 INI 适配与 `SendInput` 委托代码，不是对 AFA
源代码的复制。复刻器不修改 AFA 配置，也不在 AFA 不可用时回退这段已移除的 Rust 时序。

## ArknightsCostBarRuler

上游：`ArknightsCostBarRuler`，MIT License，Copyright (c) 2025 Z_06。

复刻器**不包含也不分发**该项目的任何代码。它作为独立进程由用户自行安装运行，
复刻器只通过其文档化的本地 API（`http://127.0.0.1:2606/` 与 `ws://127.0.0.1:2606/`，
协议见上游 `docs/API.md`）读取帧数、并发送校准/计时器控制命令。

MIT 许可证只在分发其代码或实质部分时才要求附带版权声明；本项目未分发其代码，
此处的致谢与说明属于自愿署名。若将来选择把 `ruler-core` 作为 Rust 库静态链接进来，
则必须在发行包中附带上游 `LICENSE` 全文。

> 注：`repl-frames` 中的快照字段名（`totalElapsedFrames`、`frameId`、`battleState` 等）
> 属于对外接口/协议的事实性描述，不构成对上游源代码的复制。

## 不随本仓库分发的游戏资源

复刻器需要两类来自 MAA 的资源文件，但**本仓库不包含它们**，程序在运行时从用户
自行提供的 MAA 资源目录读取（首次运行向导会引导设置路径）：

1. `resource/Arknights-Tile-Pos/`（约 82MB 关卡地图数据，含 `overview.json`）
   —— 上游为 [yuanyan3060/Arknights-Tile-Pos](https://github.com/yuanyan3060/Arknights-Tile-Pos)，
   由 MAA 二次分发。
2. `resource/template/` 下的少量 UI 图标模板
   （`BattleOpersFlag.png`、`BattleOperRole*.png`、`BattleOfficiallyBegin.png`）
   —— 派生自《明日方舟》客户端素材。

不随仓库分发这两类文件有两个理由：一是避免对上游游戏素材的版权状态做出我们无权做的
判断，二是让用户始终使用与自己 MAA 版本匹配的地图数据。

## 与游戏本体的关系

《明日方舟》及其全部美术、文本、数据资源的著作权归上海鹰角网络科技有限公司
（Hypergryph）/ Studio Montagne 所有。本项目与鹰角网络无任何隶属或授权关系。

本项目不注入游戏进程、不读写游戏内存、不修改游戏文件、不与游戏服务器通信。它只使用
操作系统提供的公开接口：屏幕捕获（Windows Graphics Capture）与输入注入
（`InjectTouchInput` / `SendInput`）。即便如此，使用自动化工具仍可能违反游戏的用户协议，
风险由使用者自行承担。
