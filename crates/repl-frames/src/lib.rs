// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors
//
// 本 crate 不包含 ArknightsCostBarRuler (MIT) 的任何代码，只按其公开文档
// (docs/API.md) 实现客户端。详见仓库根目录 THIRD-PARTY-NOTICES.md。

//! `repl-frames` —— 绝对逻辑帧的来源。
//!
//! 唯一真源是 ArknightsCostBarRuler（"尺子"）在 `127.0.0.1:2606` 上的本地 API
//! （协议见其 `docs/API.md`，`apiVersion = 2`）。尺子作为独立进程由用户自行运行。
//!
//! ```no_run
//! use std::time::Duration;
//! use repl_frames::{FrameSource, RulerClient};
//!
//! let ruler = RulerClient::connect_default();
//! let snap = ruler.wait_next(0, Duration::from_secs(2))?;
//! println!("绝对帧 = {}", snap.total_elapsed_frames);
//! # Ok::<(), repl_frames::FrameError>(())
//! ```

pub mod command;
pub mod ruler;
pub mod snapshot;
pub mod source;

pub use command::{Command, DisplayMode};
pub use ruler::{RulerClient, DEFAULT_WS_URL};
pub use snapshot::{BattleState, Profile, Snapshot};
pub use source::{ConnectionStatus, FrameError, FrameSource};
