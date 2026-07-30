// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors

//! 风险确认式继续计划。
//!
//! 继续不是把旧状态机“复活”，而是由用户冻结一个截止帧，承认该帧及之前的动作已经完成，
//! 再从第一个更晚的动作建立一台新状态机。本模块只推导动作前缀和场上状态，不执行 IO。

use std::collections::{BTreeMap, HashSet};

use crate::{ActionType, Copilot, Point};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContinuationPlan {
    pub cutoff_frame: i64,
    pub next_action_index: usize,
    pub battlefield: BTreeMap<String, Point>,
    /// 当前应在部署栏、且后续仍要部署的干员。
    pub visible_binding_names: Vec<String>,
    /// 当前按作业推导为在场、但后续还要再次部署的干员；只能从头像档案恢复。
    pub archived_binding_names: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ContinuationError {
    #[error("继续截止帧必须大于 0")]
    ZeroFrame,
    #[error("当前已到第 {cutoff_frame} 帧，作业没有后续动作")]
    NoRemainingActions { cutoff_frame: i64 },
    #[error("继续后的动作 #{index} 无法定位干员「{name}」；作业前缀没有可用的在场格子")]
    UnknownFutureTarget { index: usize, name: String },
}

impl ContinuationPlan {
    pub fn build(copilot: &Copilot, cutoff_frame: i64) -> Result<Self, ContinuationError> {
        if cutoff_frame <= 0 {
            return Err(ContinuationError::ZeroFrame);
        }
        let next_action_index = copilot
            .actions
            .partition_point(|action| action.frame <= cutoff_frame);
        if next_action_index == copilot.actions.len() {
            return Err(ContinuationError::NoRemainingActions { cutoff_frame });
        }

        let mut battlefield = BTreeMap::new();
        replay_lifecycle(
            &mut battlefield,
            copilot.actions[..next_action_index].iter(),
        );

        // 提前验证后续 Skill/Retreat 能不能找到目标，同时模拟完整生命周期。
        let mut future_state = battlefield.clone();
        for (index, action) in copilot.actions[next_action_index..].iter().enumerate() {
            let index = next_action_index + index;
            match action.kind {
                ActionType::Deploy => {
                    if let Some(location) = action.location {
                        future_state.insert(action.name.clone(), location);
                    }
                }
                ActionType::UseSkill => {
                    if action.location.is_none() && !future_state.contains_key(&action.name) {
                        return Err(ContinuationError::UnknownFutureTarget {
                            index,
                            name: action.name.clone(),
                        });
                    }
                }
                ActionType::Retreat => {
                    if action.location.is_none() && !future_state.contains_key(&action.name) {
                        return Err(ContinuationError::UnknownFutureTarget {
                            index,
                            name: action.name.clone(),
                        });
                    }
                    future_state.remove(&action.name);
                }
                ActionType::Output => {}
                _ => unreachable!("Copilot validation rejects unsupported frame actions"),
            }
        }

        let mut visible_binding_names = Vec::new();
        let mut archived_binding_names = Vec::new();
        let mut seen = HashSet::new();
        for action in &copilot.actions[next_action_index..] {
            if action.kind != ActionType::Deploy || !seen.insert(action.name.clone()) {
                continue;
            }
            if battlefield.contains_key(&action.name) {
                archived_binding_names.push(action.name.clone());
            } else {
                visible_binding_names.push(action.name.clone());
            }
        }

        Ok(Self {
            cutoff_frame,
            next_action_index,
            battlefield,
            visible_binding_names,
            archived_binding_names,
        })
    }

    pub fn assumed_action_count(&self) -> usize {
        self.next_action_index
    }

    pub fn next_action<'a>(&self, copilot: &'a Copilot) -> &'a crate::Action {
        &copilot.actions[self.next_action_index]
    }
}

fn replay_lifecycle<'a>(
    battlefield: &mut BTreeMap<String, Point>,
    actions: impl Iterator<Item = &'a crate::Action>,
) {
    for action in actions {
        match action.kind {
            ActionType::Deploy => {
                if let Some(location) = action.location {
                    battlefield.insert(action.name.clone(), location);
                }
            }
            ActionType::Retreat => {
                battlefield.remove(&action.name);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST1: &str = include_str!("../../../examples/test1.json");

    #[test]
    fn frame_eleven_assumes_deploy_and_reconstructs_myrtle() {
        let job = Copilot::parse(TEST1).unwrap();
        let plan = ContinuationPlan::build(&job, 11).unwrap();
        assert_eq!(plan.next_action_index, 1);
        assert_eq!(plan.next_action(&job).frame, 40);
        assert_eq!(plan.battlefield["桃金娘"], Point::new(3, 2));
        assert_eq!(plan.visible_binding_names, vec!["风笛"]);
        assert!(plan.archived_binding_names.is_empty());
    }

    #[test]
    fn cutoff_is_inclusive_and_keeps_same_frame_actions_together() {
        let job = Copilot::parse(TEST1).unwrap();
        assert_eq!(
            ContinuationPlan::build(&job, 1).unwrap().next_action_index,
            0
        );
        assert_eq!(
            ContinuationPlan::build(&job, 10).unwrap().next_action_index,
            1
        );
        assert_eq!(
            ContinuationPlan::build(&job, 40).unwrap().next_action_index,
            2
        );
        let at_240 = ContinuationPlan::build(&job, 240).unwrap();
        assert_eq!(at_240.next_action_index, 5);
        assert!(!at_240.battlefield.contains_key("桃金娘"));
        assert_eq!(at_240.battlefield["风笛"], Point::new(2, 3));
    }

    #[test]
    fn continuation_after_last_action_is_rejected() {
        let job = Copilot::parse(TEST1).unwrap();
        assert_eq!(
            ContinuationPlan::build(&job, 249),
            Err(ContinuationError::NoRemainingActions { cutoff_frame: 249 })
        );
    }

    #[test]
    fn future_redeploy_of_an_on_field_operator_requires_archive() {
        let job = Copilot::parse(
            r#"{
                "stage_name":"x","frame_replicator":true,
                "opers":[{"name":"A","skill":1}],
                "actions":[
                    {"type":"Deploy","frame":10,"name":"A","location":[1,1]},
                    {"type":"Retreat","frame":20,"name":"A"},
                    {"type":"Deploy","frame":30,"name":"A","location":[2,2]}
                ]
            }"#,
        )
        .unwrap();
        let plan = ContinuationPlan::build(&job, 10).unwrap();
        assert_eq!(plan.archived_binding_names, vec!["A"]);
        assert!(plan.visible_binding_names.is_empty());
    }
}
