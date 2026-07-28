# 运行期 Resume 必须等待尺子 running 回执

## 状态

已接受（2026-07-28）

## 决定

运行期 AFA `ReleasePause` 发出后，Machine 设置 `resume_in_flight`。在尺子首次报告可信
`paused == Some(false)` 前，新的 paused/unknown 样本只推进 `last_frame_id`，不更新逻辑游标或
暂停状态，`observe()` 返回 false；命令生成器在此期间只能返回 `AwaitFrame`。running 回执到来后
清除标志，按该样本的绝对 `elapsed` 恢复正常巡航。

## 原因

动作前稳定等待会让尺子管线积压大量暂停画面。它们在 Resume 派发后才完成分析，因而拥有更大的
`frame_id`，但仍描述派发前的 paused 状态。旧实现接纳第一条积压样本后把 `self.paused` 重置为
true，导致同一 cursor 重复发送 Resume；连续 `ReleasePause` 最终扰乱 AFA 状态并造成 Pulse 大幅跨帧。

## 后果

- `frame_id` 新只表示分析结果新，不能单独作为 Resume 已落地的证据。
- 每个运行区间只发送一次 Resume，直到可信 running 回执到来。
- 等待期间仍推进 `last_frame_id`，避免同一拒绝样本被循环消费。
- 日志分别记录 `runtime resume waiting for running state` 与 `runtime resume confirmed`，供实机验收。
