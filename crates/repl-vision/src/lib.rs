// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors
//
// 本 crate 移植自 MaaAssistantArknights (AGPL-3.0-only):
//   src/MaaCore/Vision/Battle/BattlefieldMatcher.cpp
//   src/MaaCore/Task/BattleHelper.cpp (analyze_oper_with_cache)
//   resource/tasks/tasks.json (几何与阈值常量)
// 详见仓库根目录 THIRD-PARTY-NOTICES.md。

//! `repl-vision` —— 部署栏识别。
//!
//! 部署栏跟踪与战前头像桥接使用纯 Rust 归一化互相关，不引入 OpenCV；战前姓名识别使用静态链接的
//! ONNX Runtime 运行 MAA PaddleOCR 模型。模板、模型和字典不随仓库分发，运行时从 MAA 资源读取。

pub mod deployment;
pub mod formation;
pub mod ncc;
pub mod templates;

pub use deployment::{analyze, battle_hud_visible, track, Card};
pub use formation::{
    bridge_formation_avatars, FormationAvatar, FormationBridgeMatch, FormationError,
    FormationResources, FormationScanReport, FormationScanRow, FormationScanner,
    FormationTaskConfig, OperatorCatalog, OperatorKey, SuggestionSource,
};
pub use ncc::{best_match, find_all, Match, Template, TemplateError};
pub use templates::{TemplateLoadError, TemplateSet};
