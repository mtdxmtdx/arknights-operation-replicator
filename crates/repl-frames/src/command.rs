// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors

//! 发往尺子的 WebSocket 控制命令，以及尺子的应答信封。
//!
//! 命令集见 ArknightsCostBarRuler `docs/API.md` 的 "WebSocket 请求" 一节。
//! 只实现本项目用得到的子集 —— 明确不实现 `exit` / `deleteProfile` 这类
//! 破坏性命令，避免误操作把用户的尺子关掉或删掉校准。

use serde_json::{json, Value};

/// 显示模式。只影响尺子 HUD 的数法，不影响 `totalElapsedFrames`。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayMode {
    ZeroToNMinusOne,
    ZeroToN,
    OneToN,
}

impl DisplayMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ZeroToNMinusOne => "0_to_n-1",
            Self::ZeroToN => "0_to_n",
            Self::OneToN => "1_to_n",
        }
    }
}

#[derive(Clone, Debug)]
pub enum Command {
    /// 主动拉一次当前快照。逐帧推进时靠它轮询新样本
    /// （尺子只在状态变化时推送，暂停期间不会自己推）。
    GetSnapshot,
    /// 查询当前战斗中 `<= frame_id` 的已分析帧。
    GetFrame { frame_id: u64 },
    /// 进入待校准。
    PrepareCalibration,
    /// 开始校准。
    StartCalibration,
    /// 取消正在进行的校准。
    CancelCalibration,
    /// 切换校准配置。
    UseProfile { filename: String },
    /// 设置 HUD 帧数显示模式。
    SetDisplayMode { mode: DisplayMode },
    /// 按帧调整计时器，可为负。
    AdjustTimer { frames: i64 },
    /// 把计时器设为绝对帧数。
    SetTimer { frames: u64 },
    /// 重置计时器，并清空当前战斗的历史帧。
    ResetTimer,
}

impl Command {
    /// 协议里的 `type` 字段。
    pub const fn action(&self) -> &'static str {
        match self {
            Self::GetSnapshot => "getSnapshot",
            Self::GetFrame { .. } => "getFrame",
            Self::PrepareCalibration => "prepareCalibration",
            Self::StartCalibration => "startCalibration",
            Self::CancelCalibration => "cancelCalibration",
            Self::UseProfile { .. } => "useProfile",
            Self::SetDisplayMode { .. } => "setDisplayMode",
            Self::AdjustTimer { .. } => "adjustTimer",
            Self::SetTimer { .. } => "setTimer",
            Self::ResetTimer => "resetTimer",
        }
    }

    /// 序列化成一条 WebSocket 文本消息。
    pub fn to_json(&self, request_id: Option<&str>) -> String {
        let mut v = json!({ "type": self.action() });
        let obj = v.as_object_mut().expect("json! built an object");
        if let Some(id) = request_id {
            obj.insert("requestId".into(), Value::String(id.to_owned()));
        }
        match self {
            Self::GetFrame { frame_id } => {
                obj.insert("frameId".into(), json!(frame_id));
            }
            Self::UseProfile { filename } => {
                obj.insert("filename".into(), json!(filename));
            }
            Self::SetDisplayMode { mode } => {
                obj.insert("displayMode".into(), json!(mode.as_str()));
            }
            Self::AdjustTimer { frames } => {
                obj.insert("frames".into(), json!(frames));
            }
            Self::SetTimer { frames } => {
                obj.insert("frames".into(), json!(frames));
            }
            Self::GetSnapshot
            | Self::PrepareCalibration
            | Self::StartCalibration
            | Self::CancelCalibration
            | Self::ResetTimer => {}
        }
        v.to_string()
    }
}

/// 尺子对一条请求的应答。
///
/// 注意：服务器**主动推送**的是裸快照对象（顶层就是快照字段），没有 `type`；
/// 只有对请求的应答才用这个信封。解析时先看有没有 `type` 字段来区分。
#[derive(Clone, Debug)]
pub enum Envelope {
    Ack {
        request_id: Option<String>,
        action: String,
    },
    Snapshot {
        request_id: Option<String>,
        payload: Value,
    },
    Frame {
        request_id: Option<String>,
        requested_frame_id: Option<u64>,
        actual_frame_id: Option<u64>,
        fell_back: bool,
        fallback_reason: Option<String>,
        frame: Value,
    },
    Error {
        request_id: Option<String>,
        code: String,
        message: String,
    },
}

/// 一条入站 WebSocket 文本消息。
#[derive(Clone, Debug)]
pub enum Inbound {
    /// 服务器主动推送的裸快照。
    Push(Value),
    /// 对请求的应答。
    Reply(Envelope),
}

/// 解析一条入站文本消息。
///
/// 返回 `None` 表示不是合法 JSON 对象（记日志丢弃即可，不应断开连接）。
pub fn parse_inbound(text: &str) -> Option<Inbound> {
    let value: Value = serde_json::from_str(text).ok()?;
    if !value.is_object() {
        return None;
    }
    let Some(kind) = value.get("type").and_then(Value::as_str) else {
        // 没有 type 字段 => 兼容旧客户端的顶层快照推送
        return Some(Inbound::Push(value));
    };
    let request_id = value
        .get("requestId")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let env = match kind {
        "ack" => Envelope::Ack {
            request_id,
            action: value
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        },
        "snapshot" => Envelope::Snapshot {
            request_id,
            payload: value.get("payload").cloned().unwrap_or(Value::Null),
        },
        "frame" => Envelope::Frame {
            request_id,
            requested_frame_id: value.get("requestedFrameId").and_then(Value::as_u64),
            actual_frame_id: value.get("actualFrameId").and_then(Value::as_u64),
            fell_back: value
                .get("fellBack")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            fallback_reason: value
                .get("fallbackReason")
                .and_then(Value::as_str)
                .map(str::to_owned),
            frame: value.get("frame").cloned().unwrap_or(Value::Null),
        },
        "error" => Envelope::Error {
            request_id,
            code: value
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_owned(),
            message: value
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        },
        other => {
            log::warn!("ruler sent unknown envelope type '{other}', ignoring");
            return None;
        }
    };
    Some(Inbound::Reply(env))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_match_api_md_examples() {
        assert_eq!(
            Command::GetSnapshot.to_json(Some("snap-1")),
            r#"{"requestId":"snap-1","type":"getSnapshot"}"#
        );
        let v: Value =
            serde_json::from_str(&Command::GetFrame { frame_id: 4241 }.to_json(Some("f"))).unwrap();
        assert_eq!(v["type"], "getFrame");
        assert_eq!(v["frameId"], 4241);

        let v: Value =
            serde_json::from_str(&Command::AdjustTimer { frames: -30 }.to_json(None)).unwrap();
        assert_eq!(v["type"], "adjustTimer");
        assert_eq!(v["frames"], -30);
        assert!(v.get("requestId").is_none());

        let v: Value = serde_json::from_str(
            &Command::SetDisplayMode {
                mode: DisplayMode::OneToN,
            }
            .to_json(None),
        )
        .unwrap();
        assert_eq!(v["displayMode"], "1_to_n");
    }

    #[test]
    fn bare_object_is_a_push() {
        let msg = parse_inbound(r#"{"isRunning":true,"totalElapsedFrames":7}"#).unwrap();
        match msg {
            Inbound::Push(v) => assert_eq!(v["totalElapsedFrames"], 7),
            other => panic!("expected push, got {other:?}"),
        }
    }

    #[test]
    fn typed_envelopes_parse() {
        let msg = parse_inbound(r#"{"type":"ack","requestId":"r1","action":"command"}"#).unwrap();
        assert!(matches!(msg, Inbound::Reply(Envelope::Ack { .. })));

        let msg = parse_inbound(r#"{"type":"snapshot","requestId":"r2","payload":{"frameId":9}}"#)
            .unwrap();
        match msg {
            Inbound::Reply(Envelope::Snapshot { payload, .. }) => assert_eq!(payload["frameId"], 9),
            other => panic!("expected snapshot reply, got {other:?}"),
        }

        let msg = parse_inbound(
            r#"{"type":"error","requestId":"r4","code":"invalid_request","message":"bad"}"#,
        )
        .unwrap();
        match msg {
            Inbound::Reply(Envelope::Error { code, message, .. }) => {
                assert_eq!(code, "invalid_request");
                assert_eq!(message, "bad");
            }
            other => panic!("expected error reply, got {other:?}"),
        }

        let msg = parse_inbound(
            r#"{"type":"frame","requestedFrameId":4241,"actualFrameId":4240,"fellBack":true,
                "fallbackReason":"frame_skipped","frame":{"frameId":4240}}"#,
        )
        .unwrap();
        match msg {
            Inbound::Reply(Envelope::Frame {
                actual_frame_id,
                fell_back,
                fallback_reason,
                ..
            }) => {
                assert_eq!(actual_frame_id, Some(4240));
                assert!(fell_back);
                assert_eq!(fallback_reason.as_deref(), Some("frame_skipped"));
            }
            other => panic!("expected frame reply, got {other:?}"),
        }
    }

    #[test]
    fn garbage_is_dropped_not_fatal() {
        assert!(parse_inbound("not json").is_none());
        assert!(parse_inbound("[1,2,3]").is_none());
        assert!(parse_inbound(r#"{"type":"someFutureThing"}"#).is_none());
    }
}
