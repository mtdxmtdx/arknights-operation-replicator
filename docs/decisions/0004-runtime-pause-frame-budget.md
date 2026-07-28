# 运行期 Pause 按逻辑帧预算确认

## 状态

已接受（2026-07-28）

## 决定

运行期 AFA `PressPause` 发出后，暂停无效窗口按尺子绝对帧 `elapsed` 相对派发帧的增长量计算，
不按新 `frame_id` 样本条数计算。同一 `elapsed` 的重复 running 样本不消耗预算。当前最多允许增长
6 帧（约 200ms）；`FRAME_LEAD=8` 必须通过编译期断言保证目标前仍有至少 2 帧余量。

## 原因

AFA 的 `ActionPressPause` 持有 ESC 50ms，之后还有游戏输入处理和尺子分析延迟。尺子约 60Hz 产生
分析样本，旧实现收到四条 running 样本（约 67ms）就中止，即使它们的 `elapsed` 没有增长，也会
误报 `PauseIneffective`。样本频率不是游戏逻辑时间，不能作为安全预算。

## 后果

- AFA Pause 有约 200ms 的真实帧落地窗口，同时不会耗尽 8 帧提前暂停区间。
- 实际前进达到 6 帧仍未暂停时，在目标前 2 帧失败关闭，不补发 Pause。
- 日志记录 Resume/Pause 单次派发、每条 pending 样本的真实 advance 和最终确认。
