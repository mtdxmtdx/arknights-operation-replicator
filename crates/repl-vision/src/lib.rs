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
//! 只需要归一化互相关模板匹配，纯 Rust 实现，不引入 OpenCV。
//! 模板 PNG 不随本仓库分发，运行时从用户配置的 MAA 资源目录读取。

pub mod deployment;
pub mod ncc;
pub mod templates;

pub use deployment::{analyze, battle_hud_visible, track, Card};
pub use ncc::{best_match, find_all, Match, Template, TemplateError};
pub use templates::{TemplateLoadError, TemplateSet};
