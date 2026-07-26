// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors

//! 帧源抽象。
//!
//! 状态机只依赖这个 trait，不直接依赖尺子的 WebSocket 实现 —— 这样离线回放
//! 测试可以塞一个假帧源进去。

use std::time::Duration;

use crate::{command::Command, snapshot::Snapshot};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionStatus {
    /// 还没连上，或连接已断开（worker 会自动重试）。
    Disconnected,
    Connecting,
    Connected,
}

impl ConnectionStatus {
    pub const fn is_connected(self) -> bool {
        matches!(self, Self::Connected)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("尺子未连接")]
    NotConnected,
    #[error("等待新分析帧超时（{0:?}）")]
    Timeout(Duration),
    #[error("发送命令失败：{0}")]
    Send(String),
    #[error("尺子返回错误 [{code}]：{message}")]
    Ruler { code: String, message: String },
}

/// 绝对逻辑帧的来源。
pub trait FrameSource: Send + Sync {
    fn status(&self) -> ConnectionStatus;

    /// 当前已知的最新快照。不阻塞；连接刚建立时可能是 `None`。
    fn latest(&self) -> Option<Snapshot>;

    /// 阻塞等待一条 `frame_id > after_frame_id` 的**新**快照。
    ///
    /// 之所以要按 `frame_id` 过滤而不是"等下一次推送"：尺子只在状态变化时推送，
    /// 暂停期间同一份快照会被反复读到，把它当成新样本会让逐帧推进空转。
    /// 实现方在有等待者时会主动 `getSnapshot` 轮询。
    fn wait_next(&self, after_frame_id: u64, timeout: Duration) -> Result<Snapshot, FrameError>;

    /// 发送一条控制命令（fire-and-forget，不等应答）。
    fn send(&self, command: Command) -> Result<(), FrameError>;
}

/// 供测试使用的假帧源：按预置的快照序列逐条吐出。
#[cfg(any(test, feature = "test-util"))]
pub mod fake {
    use std::sync::Mutex;

    use super::*;

    pub struct FakeFrameSource {
        queue: Mutex<std::collections::VecDeque<Snapshot>>,
        latest: Mutex<Option<Snapshot>>,
        pub sent: Mutex<Vec<String>>,
    }

    impl FakeFrameSource {
        pub fn new(snapshots: impl IntoIterator<Item = Snapshot>) -> Self {
            Self {
                queue: Mutex::new(snapshots.into_iter().collect()),
                latest: Mutex::new(None),
                sent: Mutex::new(Vec::new()),
            }
        }
    }

    impl FrameSource for FakeFrameSource {
        fn status(&self) -> ConnectionStatus {
            ConnectionStatus::Connected
        }

        fn latest(&self) -> Option<Snapshot> {
            self.latest.lock().unwrap().clone()
        }

        fn wait_next(
            &self,
            after_frame_id: u64,
            timeout: Duration,
        ) -> Result<Snapshot, FrameError> {
            let mut queue = self.queue.lock().unwrap();
            while let Some(s) = queue.pop_front() {
                if s.frame_id.is_some_and(|id| id > after_frame_id) {
                    *self.latest.lock().unwrap() = Some(s.clone());
                    return Ok(s);
                }
            }
            Err(FrameError::Timeout(timeout))
        }

        fn send(&self, command: Command) -> Result<(), FrameError> {
            self.sent.lock().unwrap().push(command.action().to_owned());
            Ok(())
        }
    }
}
