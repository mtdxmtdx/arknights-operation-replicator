# 作业编辑器使用说明

本文说明如何使用帧级复刻器的人工作业编辑器，以及不同类型地图应当填写的 MAA 关卡标识。
编辑器采用人工打轴：它不会监听或自动识别游戏中的操作；用户需要在时间轴上自行添加动作并填写
绝对帧。

## 1. 开始前准备

1. 启动 `repl-app.exe`，打开“作业编辑”页。
2. 在顶部“MAA 资源”一行点击“选择…”，选择 MAA 的 `resource` 文件夹；也可以选择它的上一级
   MAA 文件夹，程序会自动进入 `resource`。成功后界面显示“已加载”，路径原子保存到 EXE 旁的
   `config.json`，地图和干员搜索在当前窗口立即刷新，不需要重启。
3. 新建作业或打开已有 JSON。
4. 填写标题和关卡标识。关卡标识匹配成功后，编辑器会显示逻辑地图；匹配失败时仍可手填格子，
   但作业不能进入正常复刻。
5. 如需边看实机边打轴，启动并连接尺子。编辑页会显示当前绝对帧；“跟随尺子”只控制时间轴光标，
   不会向游戏发送任何输入。

取消选择或选择缺少 `Arknights-Tile-Pos/overview.json` 的目录不会覆盖原配置。资源目录还必须包含
可读取的 `battle_data.json`，否则界面会保留旧资源并显示加载失败。

## 2. 关卡标识填写规则

`stage_name` 可以填写 MAA `Arknights-Tile-Pos/overview.json` 中的以下任一字段：

- `stageId`：推荐，通常唯一且最稳定，例如 `main_01-07`、`act31side_09`。
- `code`：游戏界面显示的关卡号，例如 `1-7`、`HS-9`；只有确认唯一时才使用。
- `levelId`：完整内部路径，例如 `obt/main/level_main_01-07`。
- `name`：中文关卡名；只有确认唯一且没有空格、异体字问题时才使用。

建议始终复制精确的 `stageId`。同一个 `code` 可能命中普通/剧情变体，集成战略、生息演算、危机
合约和部分特殊活动还会大量复用相同 `code`，此时必须使用 `stageId`。`#f#` 是上游地图索引中的
剧情/固定编队变体；如果普通条目和 `#f#` 条目同时存在，优先填写不带 `#f#` 的普通 `stageId`。

最小示例：

```json
{
  "stage_name": "main_01-07",
  "opers": [],
  "actions": []
}
```

## 3. 全部地图类型与格式

下表覆盖当前 MAA Tile Pos 索引中的全部地图家族。`N`、`NN`、`X`、`Y`、`SS` 都是占位符，实际
填写时应从本机 `overview.json` 复制完整值，不能把占位符原样写进作业。

### 3.1 常驻关卡

| 地图类型 | 推荐 `stageId` 格式 | 示例 | 说明 |
| --- | --- | --- | --- |
| 主线普通 | `main_CC-SS` | `main_01-07` | `CC` 为章节，`SS` 为内部关卡序号 |
| 主线支线 | `sub_CC-SS` | `sub_02-01` | 游戏中通常显示为 `S2-1` 等 |
| 主线简单 | `easy_CC-SS` | `easy_09-01` | 简单环境使用独立地图条目 |
| 主线磨难 | `tough_CC-SS` | `tough_10-01` | 磨难环境使用独立地图条目 |
| 主线绝境作战 | `hard_CC-SS` | `hard_05-01` | 游戏中通常显示为 `H5-1` 等 |
| 资源保障 | `wk_armor_N` | `wk_armor_5` | `SK-N` |
| 空中威胁 | `wk_fly_N` | `wk_fly_5` | `CA-N` |
| 战术演习 | `wk_kc_N` | `wk_kc_5` | `LS-N` |
| 货物运送 | `wk_melee_N` | `wk_melee_5` | `CE-N` |
| 粉碎防御 | `wk_toxic_N` | `wk_toxic_5` | `AP-N` |
| 芯片搜索 | `pro_a_N` ～ `pro_d_N` | `pro_a_2` | 对应 `PR-A-2` ～ `PR-D-2` |
| 教学关 | `tr_NN` | `tr_01` | 对应 `TR-1` 等 |
| 剿灭作战 | `camp_NN` 或 `camp_r_NN` | `camp_01` | Tile Pos 可识别，但费用尺在剿灭中通常不适用 |
| 干员悖论模拟 | `mem_<干员内部名>_N` | `mem_12fce_1` | 推荐直接从索引复制，不要自行推导内部名 |
| 系统引导图 | `guide_NN` | `guide_01` | 主要用于内部引导，不建议制作常规复刻作业 |

### 3.2 常规活动关卡

| 地图类型 | 推荐 `stageId` 格式 | 示例 | 说明 |
| --- | --- | --- | --- |
| SideStory / 插曲 | `actNNside_SS` | `act31side_09`（HS-9） | 普通、EX、S、MO 关均以索引实际值为准 |
| 故事集 | `actNNmini_SS` | `act20mini_01`（CG-1） | 部分条目同时存在 `#f#` 变体 |
| 早期/复刻活动 | `actNNdX_SS` | `act11d0_01`（TW-1） | `d0`、`d3`、`d5` 等是活动内部代号 |
| 更早期活动 | `aNNN_SS` | `a001_01`（GT-1） | 早期活动可能使用 `a001`、`a003` 等前缀 |

活动显示关卡号不一定能反推出 `stageId`。例如 HS-9 应填写 `act31side_09`；不要根据 `HS-9`
自行拼接成不存在的 `hs_09`。

### 3.3 危机合约

| 地图类型 | 推荐 `stageId` 格式 | 示例 |
| --- | --- | --- |
| 旧危机合约 | `level_rune_NN-SS` | `level_rune_11-01` |
| 新危机合约 | `level_crisis_v2_NN-SS` | `level_crisis_v2_05-01` |
| 活动化危机合约 | `level_actNNrune_NN-SS` | `level_act38rune_01-01` |

危机合约的地区名和地图名可能重复，应使用精确 `stageId`，不要填写“炎国”“维多利亚”等地区名。

### 3.4 集成战略

普通作业编辑器使用 Tile Pos 的内部标识：

```text
ro<主题编号>_<关卡类别>_<序号>
```

示例：

```json
{ "stage_name": "ro1_b_1" }
```

它对应集成战略地图“开门请当心”。当前索引包含 `ro1` ～ `ro5`；具体条目还可能出现更多层级或
字母后缀，例如 `ro1_e_1_1`、`ro3_b_1_b`，因此必须从索引复制完整 `stageId`。

不要填写 `ISW-NO` 或 `ISW-DF`：这些 `code` 被大量地图复用，无法唯一定位。MAA 自身
`resource/roguelike/<主题>/autopilot/` 中使用中文 `stage_name` 和 `_collapse` 文件的规则属于
集成战略自动化内部格式，不是本编辑器应当填写的 Tile Pos 标识。

### 3.5 生息演算、保全派驻与其他特殊模式

| 地图类型 | 推荐 `stageId` 格式 | 示例 | 说明 |
| --- | --- | --- | --- |
| 生息演算 | `sandbox_<主题>_<序号>` | `sandbox_1_01` | `RA-NO` 会重复，必须用 `stageId` |
| 保全派驻 | `lt_NN_NN` | `lt_01_01` | 教学条目可能为 `lt_tr_*` |
| 引航者试炼 | `actNbossrush_NN` | `act1bossrush_01` | `TN-1` 等会跨期重复 |
| 多人/协作活动 | `actNmulti_NN`、`actNvmulti-NN` | `act1multi_01` | 不同视角或期数可能共用显示关卡号 |
| 竞技/特殊对抗 | `actNfootball_NN`、`actNenemyduel_*`、`actNarcade_*` | `act1football_01` | 精确格式以索引为准 |
| 自走棋类 | `actNautochess_*`、`actNvautochess_*` | `act1autochess_m01` | 模式名和关卡号通常不唯一 |
| 特殊活动玩法 | `actNbreak_NN`、`actNvecb_NN`、`actNhalfidle_NN` | `act1break_01` | 还可能随新活动增加新前缀 |

“索引能加载地图”只表示编辑器能得到逻辑格子，并不保证该玩法适合当前的费用尺和 AFA 输入链路。
剿灭、多人玩法、自走棋、费用不正常回复、费用已满或费用锁定的场景通常不能满足帧源前提；应先用
尺子确认绝对帧能够稳定、单调推进，再制作和运行作业。

## 4. 查找准确关卡标识

### 方法一：查询 MAA 索引

在 PowerShell 中执行，替换 `$needle`：

```powershell
$resource = 'D:\path\to\MAA\resource'
$needle = 'HS-9'
$overview = Get-Content -LiteralPath (Join-Path $resource 'Arknights-Tile-Pos\overview.json') -Raw -Encoding UTF8 |
    ConvertFrom-Json
$overview.PSObject.Properties.Value |
    Where-Object {
        $_.stageId -like "*$needle*" -or
        $_.code -like "*$needle*" -or
        $_.name -like "*$needle*" -or
        $_.levelId -like "*$needle*"
    } |
    Select-Object stageId, code, name, levelId
```

从结果的 `stageId` 列复制所需值。如果出现多条结果，结合游戏中的地图名称和活动确认，不要直接用
重复的 `code`。

### 方法二：使用 `tile-preview`

```powershell
.\target\release\tile-preview.exe HS-9
```

工具会尝试解析标识；失败时会打印相近候选。成功后还会生成带逻辑格子的预览图，可用于核对地图。

## 5. 编队与动作编辑

1. 在“编队干员”中输入名称片段，从候选中选择开局编队成员；召唤物和后续出现的目标放入延迟目标。
2. 移动时间轴光标到目标绝对帧，添加动作：
   - **部署**：选择目标、地图格子和上/下/左/右/无方向。
   - **技能/装置**：普通干员可只选名称；地图装置应填写逻辑格子，显式格子优先。
   - **撤退**：选择已经部署的目标；存在重名目标时建议同时保留明确位置语义。
   - **注释**：只记录说明，不进入复刻执行动作。
3. 数字和文本需要按回车或点击输入框外才提交；撤销会以一次确认输入为一个步骤。
4. 同帧动作按列表顺序执行，可在时间轴中调整顺序。
5. 处理底部诊断。草稿可以保存，但存在错误时不能切换到复刻运行。

## 6. 保存、运行与安全检查

1. 使用“保存”或“另存为”写出 JSON；首次建议保留一份独立测试作业。
2. 切换到“复刻执行”后重新加载该作业，确认标题、关卡、编队和动作数量正确。
3. 进入关卡并等待 AFA 在第 0 帧自动暂停；确认尺子为可信 `1x_paused` 后开始。
4. 完成焦点交接和可见卡片绑定。开局不可见的召唤物会在首次部署目标帧延迟绑定。
5. 最后动作完成后程序保持在目标帧，不自动恢复游戏。

作业格式和完整 JSON 示例见 [`../examples/sample-job.json`](../examples/sample-job.json)，实机检查项见
[`EDITOR_ACCEPTANCE.md`](EDITOR_ACCEPTANCE.md)。
