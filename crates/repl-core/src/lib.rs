// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors
//
// 本 crate 含有移植自 MaaAssistantArknights (AGPL-3.0-only) 与
// arknights-frame-assistant (GPL-3.0-only) 的代码。逐文件来源见
// 仓库根目录 THIRD-PARTY-NOTICES.md。

//! `repl-core` —— 帧级操作复刻器的纯逻辑层。
//!
//! 这个 crate **不做任何 IO**：没有截图、没有输入注入、没有网络。它只包含
//! 可以离线单元测试的东西：
//!
//! - [`geom`] 共享几何词汇（格子坐标 / 1280×720 参考像素坐标）
//! - `copilot` 作业模型（MAA copilot schema 的超集，动作条件换成绝对逻辑帧）
//! - `level` / `tile` 关卡数据与地图投影（移植自 MAA 的 `Arknights-Tile-Pos`）
//! - `viewport` 参考坐标系 ↔ 真实客户区像素
//! - `gesture` 部署拖拽手势（移植自 MAA 的 `BattleHelper::deploy_oper`）
//! - `machine` 帧复刻状态机（吃尺子快照，吐注入指令）

pub mod continuation;
pub mod copilot;
pub mod geom;
pub mod gesture;
pub mod job_document;
pub mod level;
pub mod machine;
pub mod stepping;
pub mod tile;
pub mod viewport;

pub use continuation::{ContinuationError, ContinuationPlan};
pub use copilot::{Action, ActionType, AfterLastAction, Copilot, CopilotError};
pub use geom::{Direction, Placement, Point, Rect, Role, REF_ASPECT, REF_HEIGHT, REF_WIDTH};
pub use gesture::{deploy, DeployGesture, Swipe};
pub use job_document::{
    ActionDraft, ActionId, Diagnostic, DiagnosticSeverity, JobDocument, JobDocumentError,
};
pub use level::{Level, LevelError, LevelPack, TileKey};
pub use machine::{AbortReason, Command, Completion, FrameView, Machine, Phase};
pub use stepping::GapTuner;
pub use tile::TileProjection;
pub use viewport::{HAnchor, VAnchor, Viewport};

/// 游戏在 1 倍速下的逻辑帧率。
///
/// 依据：arknights-frame-assistant README —— "前进 33ms：前进游戏 1 倍速下的
/// 1 逻辑帧（游戏为 1 秒 30 逻辑帧）"。2 倍速为 16ms/帧，部署慢放 0.2 倍速为
/// 166ms/帧。
pub const LOGICAL_FPS_1X: f64 = 30.0;

/// 战斗速度档位。首版只支持 [`Speed::One`]。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Speed {
    /// 0.2 倍速：选中干员或拖拽待部署卡片时的慢放。
    PointTwo,
    #[default]
    One,
    Two,
}

impl Speed {
    /// 一个逻辑帧对应的墙钟毫秒数。
    pub const fn frame_millis(self) -> f64 {
        match self {
            Self::PointTwo => 1000.0 / 6.0, // ≈166ms
            Self::One => 1000.0 / 30.0,     // ≈33ms
            Self::Two => 1000.0 / 60.0,     // ≈16ms
        }
    }
}
