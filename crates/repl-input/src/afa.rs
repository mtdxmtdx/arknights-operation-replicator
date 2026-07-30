// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors

//! AFA 的只读配置适配与热键委托。
//!
//! 复刻器不修改 AFA 的配置，也不复制 AFA 的技能/撤退时序。这里唯一做的事情是
//! 读取 AFA 已注册的热键，确认 AFA 进程仍在同等权限下运行，然后发送一次热键事件。

use std::{
    collections::HashMap,
    env, fs, io,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use thiserror::Error;

use crate::{key_tap, keys::DEFAULT_TAP_HOLD, mouse, KeyCode};

// AFA `Config._DefaultHotkeys` / `GetHotkey` 的协议键名；值仍始终从用户 INI 读取。
const PRESS_PAUSE: &str = "PressPause";
const RELEASE_PAUSE: &str = "ReleasePause";
const PAUSE_SKILL: &str = "PauseSkill";
const PAUSE_RETREAT: &str = "PauseRetreat";
// AFA 发布工作流的固定资产名；进程必须以同等管理员权限运行。
const AFA_PROCESS: &str = "AFA.exe";

type IniValues = HashMap<(String, String), String>;

/// AFA 中由复刻器委托的动作。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AfaAction {
    /// AFA 的“按下暂停”热键；它是一次独立 tap，不是另一个键的 key-down。
    PressPause,
    /// AFA 的“松开暂停”热键；它是一次独立 tap，不是 PressPause 的 key-up。
    ReleasePause,
    PauseSkill,
    PauseRetreat,
}

impl AfaAction {}

/// AFA 热键解析结果。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AfaHotkey {
    Keyboard(KeyCode),
    XButton(mouse::XButton),
    Wheel(mouse::WheelDirection),
}

impl std::fmt::Display for AfaHotkey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Keyboard(key) => write!(f, "{key}"),
            Self::XButton(button) => write!(f, "{button:?}"),
            Self::Wheel(direction) => write!(f, "{direction:?}"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AfaBindings {
    pub press_pause: AfaHotkey,
    pub release_pause: AfaHotkey,
    pub pause_skill: AfaHotkey,
    pub pause_retreat: AfaHotkey,
}

impl AfaBindings {
    fn from_ini(values: &IniValues) -> Result<Self, AfaError> {
        let mut bindings = Vec::new();
        for key in [PRESS_PAUSE, RELEASE_PAUSE, PAUSE_SKILL, PAUSE_RETREAT] {
            let value = values
                .get(&("Hotkeys".to_owned(), key.to_owned()))
                .map(String::as_str)
                .unwrap_or("");
            if value.trim().is_empty() {
                return Err(AfaError::MissingHotkey { key });
            }
            let hotkey = parse_hotkey(value).map_err(|reason| AfaError::UnsupportedHotkey {
                key,
                value: value.to_owned(),
                reason,
            })?;
            if bindings.iter().any(|(_, existing)| *existing == hotkey) {
                return Err(AfaError::DuplicateHotkey {
                    value: value.to_owned(),
                });
            }
            bindings.push((key, hotkey));
        }
        Ok(Self {
            press_pause: bindings[0].1,
            release_pause: bindings[1].1,
            pause_skill: bindings[2].1,
            pause_retreat: bindings[3].1,
        })
    }

    fn get(&self, action: AfaAction) -> AfaHotkey {
        match action {
            AfaAction::PressPause => self.press_pause,
            AfaAction::ReleasePause => self.release_pause,
            AfaAction::PauseSkill => self.pause_skill,
            AfaAction::PauseRetreat => self.pause_retreat,
        }
    }
}

#[derive(Clone, Debug)]
pub struct AfaController {
    settings_path: PathBuf,
    bindings: AfaBindings,
    fingerprint: FileFingerprint,
    process_id: u32,
}

impl AfaController {
    /// 读取 AFA 配置并确认官方 AFA.exe 已以管理员权限运行。
    pub fn discover() -> Result<Self, AfaError> {
        Self::discover_inner(true)
    }

    fn discover_inner(log_bindings: bool) -> Result<Self, AfaError> {
        let settings_path = settings_path()?;
        let (values, fingerprint) = read_ini(&settings_path)?;
        validate_main_settings(&values)?;
        let bindings = AfaBindings::from_ini(&values)?;
        let process_id = validate_afa_process(None, find_afa_process())?;
        if log_bindings {
            log::info!(
                "AFA 热键解析：{PRESS_PAUSE}={}, {RELEASE_PAUSE}={}, {PAUSE_SKILL}={}, {PAUSE_RETREAT}={} (source={})",
                bindings.press_pause,
                bindings.release_pause,
                bindings.pause_skill,
                bindings.pause_retreat,
                settings_path.display(),
            );
            log::info!(
                "AFA 预检通过：process={AFA_PROCESS} pid={process_id} elevated=true settings={}",
                settings_path.display(),
            );
        }
        Ok(Self {
            settings_path,
            bindings,
            fingerprint,
            process_id,
        })
    }

    pub fn settings_path(&self) -> &Path {
        &self.settings_path
    }

    pub fn bindings(&self) -> &AfaBindings {
        &self.bindings
    }

    /// 在每次发送前重新确认 AFA 身份与配置没有变化。
    pub fn revalidate(&self) -> Result<(), AfaError> {
        let current = file_fingerprint(&self.settings_path)?;
        if current != self.fingerprint {
            return Err(AfaError::ConfigChanged);
        }
        validate_afa_process(Some(self.process_id), find_afa_process()).map(|_| ())
    }

    /// 委托一次 AFA 热键；调用方负责在此之前确认游戏窗口位于前台。
    pub fn dispatch(&self, action: AfaAction) -> Result<(), AfaError> {
        self.revalidate()?;
        match self.bindings.get(action) {
            AfaHotkey::Keyboard(key) => key_tap(key, DEFAULT_TAP_HOLD).map_err(AfaError::Input),
            AfaHotkey::XButton(button) => {
                mouse::xbutton_tap(button, DEFAULT_TAP_HOLD).map_err(AfaError::Input)
            }
            AfaHotkey::Wheel(direction) => mouse::wheel_tick(direction).map_err(AfaError::Input),
        }
    }

    /// 只读探测，用于 UI 状态灯；不会发送任何输入。
    pub fn probe() -> AfaStatus {
        match Self::discover_inner(false) {
            Ok(controller) => AfaStatus {
                ready: true,
                detail: format!("已连接（{}）", controller.settings_path.display()),
            },
            Err(error) => AfaStatus {
                ready: false,
                detail: error.to_string(),
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AfaStatus {
    pub ready: bool,
    pub detail: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileFingerprint {
    len: u64,
    modified_ns: u128,
}

#[derive(Debug, Error)]
pub enum AfaError {
    #[error("找不到 APPDATA，无法定位 AFA Settings.ini")]
    NoAppData,
    #[error("读取 AFA 配置失败 {path}: {source}")]
    ReadConfig {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("AFA 配置文件不存在或无法读取元数据 {path}: {source}")]
    ConfigMetadata {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("AFA 配置缺少 [Main]/{key} 或该设置不安全")]
    InvalidMainSetting { key: &'static str },
    #[error("AFA 热键 {key} 未绑定")]
    MissingHotkey { key: &'static str },
    #[error("AFA 热键 {key}={value} 不受支持：{reason}")]
    UnsupportedHotkey {
        key: &'static str,
        value: String,
        reason: String,
    },
    #[error("AFA 热键重复：{value}")]
    DuplicateHotkey { value: String },
    #[error("AFA.exe 未运行，请先启动 AFA")]
    NotRunning,
    #[error("AFA.exe 未以管理员权限运行")]
    NotElevated,
    #[error("AFA 进程在运行期间发生变化，请重新开始")]
    ProcessChanged,
    #[error("AFA Settings.ini 在运行期间发生变化，请重新开始")]
    ConfigChanged,
    #[error("AFA 热键输入失败：{0}")]
    Input(crate::InputError),
}

fn settings_path() -> Result<PathBuf, AfaError> {
    let appdata = env::var_os("APPDATA").ok_or(AfaError::NoAppData)?;
    Ok(PathBuf::from(appdata)
        .join("ArknightsFrameAssistant")
        .join("PC")
        .join("Settings.ini"))
}

fn read_ini(path: &Path) -> Result<(IniValues, FileFingerprint), AfaError> {
    let bytes = fs::read(path).map_err(|source| AfaError::ReadConfig {
        path: path.to_owned(),
        source,
    })?;
    let contents = decode_ini_text(&bytes).map_err(|source| AfaError::ReadConfig {
        path: path.to_owned(),
        source,
    })?;
    let mut section = String::new();
    let mut values = HashMap::new();
    for raw in contents.trim_start_matches('\u{feff}').lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            section = name.trim().to_owned();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        values.insert(
            (section.clone(), key.trim().to_owned()),
            value.trim().to_owned(),
        );
    }
    Ok((values, file_fingerprint(path)?))
}

fn decode_ini_text(bytes: &[u8]) -> io::Result<String> {
    let (encoding, payload) = if let Some(payload) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        (Some(true), payload)
    } else if let Some(payload) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        (Some(false), payload)
    } else {
        (None, bytes)
    };

    let Some(little_endian) = encoding else {
        return String::from_utf8(payload.to_vec())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error));
    };
    if payload.len() % 2 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "UTF-16 AFA Settings.ini has an odd byte length",
        ));
    }
    let words = payload.chunks_exact(2).map(|pair| {
        if little_endian {
            u16::from_le_bytes([pair[0], pair[1]])
        } else {
            u16::from_be_bytes([pair[0], pair[1]])
        }
    });
    String::from_utf16(&words.collect::<Vec<_>>())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn file_fingerprint(path: &Path) -> Result<FileFingerprint, AfaError> {
    let metadata = fs::metadata(path).map_err(|source| AfaError::ConfigMetadata {
        path: path.to_owned(),
        source,
    })?;
    let modified_ns = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_nanos());
    Ok(FileFingerprint {
        len: metadata.len(),
        modified_ns,
    })
}

fn validate_main_settings(values: &IniValues) -> Result<(), AfaError> {
    for (key, expected) in [("AutoBeginPause", "1"), ("DefaultStrongHoldProtocol", "0")] {
        if values
            .get(&("Main".to_owned(), key.to_owned()))
            .map(String::as_str)
            != Some(expected)
        {
            return Err(AfaError::InvalidMainSetting { key });
        }
    }
    Ok(())
}

fn validate_afa_process(
    expected_pid: Option<u32>,
    actual: Option<(u32, bool)>,
) -> Result<u32, AfaError> {
    let Some((pid, elevated)) = actual else {
        return Err(AfaError::NotRunning);
    };
    if !elevated {
        return Err(AfaError::NotElevated);
    }
    if expected_pid.is_some_and(|expected| expected != pid) {
        return Err(AfaError::ProcessChanged);
    }
    Ok(pid)
}

fn parse_hotkey(value: &str) -> Result<AfaHotkey, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("为空".into());
    }
    if value.chars().any(|ch| {
        matches!(
            ch,
            '~' | '*' | '$' | '!' | '^' | '+' | '#' | '&' | '<' | '>' | '(' | ')'
        )
    }) || value.contains(' ')
    {
        return Err("首版只接受单个无修饰热键".into());
    }
    let lower = value.to_ascii_lowercase();
    match lower.as_str() {
        "xbutton1" => Ok(AfaHotkey::XButton(mouse::XButton::XButton1)),
        "xbutton2" => Ok(AfaHotkey::XButton(mouse::XButton::XButton2)),
        "wheelup" => Ok(AfaHotkey::Wheel(mouse::WheelDirection::Up)),
        "wheeldown" => Ok(AfaHotkey::Wheel(mouse::WheelDirection::Down)),
        "wheelleft" => Ok(AfaHotkey::Wheel(mouse::WheelDirection::Left)),
        "wheelright" => Ok(AfaHotkey::Wheel(mouse::WheelDirection::Right)),
        "escape" | "esc" => Ok(AfaHotkey::Keyboard(KeyCode::ESCAPE)),
        "space" => Ok(AfaHotkey::Keyboard(KeyCode::SPACE)),
        "tab" => Ok(AfaHotkey::Keyboard(KeyCode::TAB)),
        "enter" | "return" => Ok(AfaHotkey::Keyboard(KeyCode::ENTER)),
        "backspace" => Ok(AfaHotkey::Keyboard(KeyCode::BACKSPACE)),
        "left" => Ok(AfaHotkey::Keyboard(KeyCode(0x25))),
        "up" => Ok(AfaHotkey::Keyboard(KeyCode(0x26))),
        "right" => Ok(AfaHotkey::Keyboard(KeyCode(0x27))),
        "down" => Ok(AfaHotkey::Keyboard(KeyCode(0x28))),
        _ if lower.len() > 1 && lower.starts_with('f') => lower[1..]
            .parse::<u8>()
            .ok()
            .and_then(KeyCode::function)
            .map(AfaHotkey::Keyboard)
            .ok_or_else(|| "无效的功能键".into()),
        _ if value.chars().count() == 1 => value
            .chars()
            .next()
            .and_then(KeyCode::from_ascii)
            .map(AfaHotkey::Keyboard)
            .ok_or_else(|| "无效的单字符键".into()),
        _ => Err("无法解析为支持的单键热键".into()),
    }
}

#[cfg(windows)]
unsafe fn token_is_elevated(token: windows::Win32::Foundation::HANDLE) -> bool {
    use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION};

    let mut elevation = TOKEN_ELEVATION::default();
    let mut size = 0u32;
    GetTokenInformation(
        token,
        TokenElevation,
        Some(std::ptr::from_mut(&mut elevation).cast()),
        std::mem::size_of::<TOKEN_ELEVATION>() as u32,
        &mut size,
    )
    .is_ok()
        && elevation.TokenIsElevated != 0
}

#[cfg(windows)]
fn find_afa_process() -> Option<(u32, bool)> {
    use windows::Win32::{
        Foundation::{CloseHandle, HANDLE},
        Security::TOKEN_QUERY,
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
                TH32CS_SNAPPROCESS,
            },
            Threading::{OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION},
        },
    };

    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut ok = Process32FirstW(snapshot, &mut entry).is_ok();
        while ok {
            let end = entry
                .szExeFile
                .iter()
                .position(|&ch| ch == 0)
                .unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..end]);
            if name.eq_ignore_ascii_case(AFA_PROCESS) {
                let process = OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION,
                    false,
                    entry.th32ProcessID,
                )
                .ok()?;
                let mut token = HANDLE::default();
                let elevated = OpenProcessToken(process, TOKEN_QUERY, &mut token).is_ok()
                    && token_is_elevated(token);
                let _ = CloseHandle(token);
                let _ = CloseHandle(process);
                let _ = CloseHandle(snapshot);
                return Some((entry.th32ProcessID, elevated));
            }
            ok = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
        None
    }
}

#[cfg(not(windows))]
fn find_afa_process() -> Option<(u32, bool)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn valid_hotkey_values() -> HashMap<(String, String), String> {
        [
            (PRESS_PAUSE, "g"),
            (RELEASE_PAUSE, "Space"),
            (PAUSE_SKILL, "XButton2"),
            (PAUSE_RETREAT, "XButton1"),
        ]
        .into_iter()
        .map(|(key, value)| (("Hotkeys".to_owned(), key.to_owned()), value.to_owned()))
        .collect()
    }

    fn valid_main_values() -> HashMap<(String, String), String> {
        [("AutoBeginPause", "1"), ("DefaultStrongHoldProtocol", "0")]
            .into_iter()
            .map(|(key, value)| (("Main".to_owned(), key.to_owned()), value.to_owned()))
            .collect()
    }

    fn temp_ini(contents: &str) -> PathBuf {
        static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(0);
        let serial = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "replicator-afa-test-{}-{serial}.ini",
            std::process::id()
        ));
        std::fs::write(&path, contents).expect("write temporary INI");
        path
    }

    fn temp_ini_bytes(contents: &[u8]) -> PathBuf {
        static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(0);
        let serial = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "replicator-afa-encoded-test-{}-{serial}.ini",
            std::process::id()
        ));
        std::fs::write(&path, contents).expect("write encoded temporary INI");
        path
    }

    fn missing_temp_path() -> PathBuf {
        static NEXT_MISSING_FILE: AtomicU64 = AtomicU64::new(0);
        let serial = NEXT_MISSING_FILE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "replicator-afa-missing-{}-{serial}.ini",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn parses_keyboard_xbutton_and_wheel_hotkeys() {
        assert_eq!(
            parse_hotkey("g"),
            Ok(AfaHotkey::Keyboard(KeyCode::from_ascii('g').unwrap()))
        );
        assert_eq!(
            parse_hotkey("f"),
            Ok(AfaHotkey::Keyboard(KeyCode::from_ascii('f').unwrap()))
        );
        assert_eq!(
            parse_hotkey("Space"),
            Ok(AfaHotkey::Keyboard(KeyCode::SPACE))
        );
        assert_eq!(
            parse_hotkey("XButton2"),
            Ok(AfaHotkey::XButton(mouse::XButton::XButton2))
        );
        assert_eq!(
            parse_hotkey("WheelLeft"),
            Ok(AfaHotkey::Wheel(mouse::WheelDirection::Left))
        );
    }

    #[test]
    fn parses_all_four_configured_bindings() {
        let bindings = AfaBindings::from_ini(&valid_hotkey_values()).expect("valid bindings");

        assert_eq!(
            bindings.press_pause,
            AfaHotkey::Keyboard(KeyCode::from_ascii('g').unwrap())
        );
        assert_eq!(bindings.release_pause, AfaHotkey::Keyboard(KeyCode::SPACE));
        assert_eq!(
            bindings.pause_skill,
            AfaHotkey::XButton(mouse::XButton::XButton2)
        );
        assert_eq!(
            bindings.pause_retreat,
            AfaHotkey::XButton(mouse::XButton::XButton1)
        );

        assert_eq!(bindings.get(AfaAction::PressPause), bindings.press_pause);
        assert_eq!(
            bindings.get(AfaAction::ReleasePause),
            bindings.release_pause
        );
        assert_eq!(bindings.get(AfaAction::PauseSkill), bindings.pause_skill);
        assert_eq!(
            bindings.get(AfaAction::PauseRetreat),
            bindings.pause_retreat
        );
    }

    #[test]
    fn rejects_each_missing_or_blank_binding() {
        for missing in [PRESS_PAUSE, RELEASE_PAUSE, PAUSE_SKILL, PAUSE_RETREAT] {
            let mut values = valid_hotkey_values();
            values.remove(&("Hotkeys".to_owned(), missing.to_owned()));
            assert!(matches!(
                AfaBindings::from_ini(&values),
                Err(AfaError::MissingHotkey { key }) if key == missing
            ));

            let mut values = valid_hotkey_values();
            values.insert(
                ("Hotkeys".to_owned(), missing.to_owned()),
                "  \t".to_owned(),
            );
            assert!(matches!(
                AfaBindings::from_ini(&values),
                Err(AfaError::MissingHotkey { key }) if key == missing
            ));
        }
    }

    #[test]
    fn rejects_duplicate_bindings_after_normalization() {
        let mut values = valid_hotkey_values();
        values.insert(
            ("Hotkeys".to_owned(), RELEASE_PAUSE.to_owned()),
            "  G  ".to_owned(),
        );

        assert!(matches!(
            AfaBindings::from_ini(&values),
            Err(AfaError::DuplicateHotkey { value }) if value == "  G  "
        ));
    }

    #[test]
    fn rejects_unsupported_binding_with_source_context() {
        let mut values = valid_hotkey_values();
        values.insert(
            ("Hotkeys".to_owned(), PAUSE_SKILL.to_owned()),
            "Shift+F1".to_owned(),
        );

        assert!(matches!(
            AfaBindings::from_ini(&values),
            Err(AfaError::UnsupportedHotkey { key, value, .. })
                if key == PAUSE_SKILL && value == "Shift+F1"
        ));
    }

    #[test]
    fn rejects_modifiers_and_sequences() {
        assert!(parse_hotkey("^g").is_err());
        assert!(parse_hotkey("Shift+F1").is_err());
        assert!(parse_hotkey("Button 1").is_err());
    }

    #[test]
    fn hotkey_parser_normalizes_case_and_outer_whitespace() {
        assert_eq!(
            parse_hotkey("  G  "),
            Ok(AfaHotkey::Keyboard(KeyCode::from_ascii('g').unwrap()))
        );
        assert_eq!(
            parse_hotkey(" xBuTtOn2 "),
            Ok(AfaHotkey::XButton(mouse::XButton::XButton2))
        );
        assert_eq!(
            parse_hotkey(" WHEELUP "),
            Ok(AfaHotkey::Wheel(mouse::WheelDirection::Up))
        );
        assert_eq!(
            parse_hotkey("esc"),
            Ok(AfaHotkey::Keyboard(KeyCode::ESCAPE))
        );
        assert_eq!(
            parse_hotkey("return"),
            Ok(AfaHotkey::Keyboard(KeyCode::ENTER))
        );
    }

    #[test]
    fn hotkey_parser_checks_function_key_and_input_boundaries() {
        assert_eq!(
            parse_hotkey("F1"),
            Ok(AfaHotkey::Keyboard(KeyCode::function(1).unwrap()))
        );
        assert_eq!(
            parse_hotkey("f24"),
            Ok(AfaHotkey::Keyboard(KeyCode::function(24).unwrap()))
        );
        assert!(parse_hotkey("F0").is_err());
        assert!(parse_hotkey("F25").is_err());
        assert!(parse_hotkey("").is_err());
        assert!(parse_hotkey(" ").is_err());
        assert!(parse_hotkey("g h").is_err());
        assert!(parse_hotkey("€").is_err());
    }

    #[test]
    fn main_settings_require_afa_opening_pause_and_disable_strong_hold() {
        assert!(validate_main_settings(&valid_main_values()).is_ok());

        for (invalid, wrong_value) in [("AutoBeginPause", "0"), ("DefaultStrongHoldProtocol", "1")]
        {
            let mut missing = valid_main_values();
            missing.remove(&("Main".to_owned(), invalid.to_owned()));
            assert!(matches!(
                validate_main_settings(&missing),
                Err(AfaError::InvalidMainSetting { key }) if key == invalid
            ));

            let mut enabled = valid_main_values();
            enabled.insert(
                ("Main".to_owned(), invalid.to_owned()),
                wrong_value.to_owned(),
            );
            assert!(matches!(
                validate_main_settings(&enabled),
                Err(AfaError::InvalidMainSetting { key }) if key == invalid
            ));
        }
    }

    #[test]
    fn process_validation_is_fail_closed_and_checks_identity() {
        assert!(matches!(
            validate_afa_process(None, None),
            Err(AfaError::NotRunning)
        ));
        assert!(matches!(
            validate_afa_process(None, Some((42, false))),
            Err(AfaError::NotElevated)
        ));
        assert!(matches!(
            validate_afa_process(Some(7), Some((42, true))),
            Err(AfaError::ProcessChanged)
        ));
        assert!(matches!(
            validate_afa_process(None, Some((42, true))),
            Ok(42)
        ));
        assert!(matches!(
            validate_afa_process(Some(42), Some((42, true))),
            Ok(42)
        ));
    }

    #[test]
    fn read_ini_handles_bom_comments_spacing_and_malformed_lines() {
        let path = temp_ini(
            "\u{feff}; comment\n# another comment\n\n [ Main ]\nAutoBeginPause = 1 \nDefaultStrongHoldProtocol= 0\nignored line\n[ Hotkeys ]\n PressPause = G \nReleasePause=Space\nPauseSkill = XButton2\nPauseRetreat = WheelLeft\n",
        );

        let result = read_ini(&path);
        let _ = std::fs::remove_file(&path);
        let (values, fingerprint) = result.expect("parse temporary INI");

        assert_eq!(
            values.get(&("Main".into(), "AutoBeginPause".into())),
            Some(&"1".into())
        );
        assert_eq!(
            values.get(&("Main".into(), "DefaultStrongHoldProtocol".into())),
            Some(&"0".into())
        );
        assert_eq!(
            values.get(&("Hotkeys".into(), "PressPause".into())),
            Some(&"G".into())
        );
        assert_eq!(
            values.get(&("Hotkeys".into(), "PauseRetreat".into())),
            Some(&"WheelLeft".into())
        );
        assert!(fingerprint.len > 0);
    }

    #[test]
    fn read_ini_accepts_utf16_bom_files_written_by_afa() {
        let contents = "[Main]\r\nAutoBeginPause=1\r\nDefaultStrongHoldProtocol=0\r\n\
                        [Hotkeys]\r\nPressPause=g\r\nReleasePause=Space\r\n\
                        PauseSkill=XButton2\r\nPauseRetreat=XButton1\r\n";

        for (bom, words) in [
            ([0xFF, 0xFE], contents.encode_utf16().collect::<Vec<_>>()),
            ([0xFE, 0xFF], contents.encode_utf16().collect::<Vec<_>>()),
        ] {
            let mut bytes = bom.to_vec();
            if bom == [0xFF, 0xFE] {
                bytes.extend(words.iter().flat_map(|word| word.to_le_bytes()));
            } else {
                bytes.extend(words.iter().flat_map(|word| word.to_be_bytes()));
            }
            let path = temp_ini_bytes(&bytes);
            let result = read_ini(&path);
            let _ = std::fs::remove_file(&path);
            let (values, _) = result.expect("parse UTF-16 AFA INI");

            validate_main_settings(&values).expect("validate main settings");
            AfaBindings::from_ini(&values).expect("parse all four AFA bindings");
        }
    }

    #[test]
    fn missing_ini_and_metadata_are_reported_as_distinct_errors() {
        let path = missing_temp_path();
        assert!(matches!(read_ini(&path), Err(AfaError::ReadConfig { .. })));
        assert!(matches!(
            file_fingerprint(&path),
            Err(AfaError::ConfigMetadata { .. })
        ));
    }

    #[test]
    fn revalidate_rejects_a_changed_settings_file_before_process_lookup() {
        let path = temp_ini("[Main]\nAutoBeginPause=1\n");
        let fingerprint = file_fingerprint(&path).expect("fingerprint temporary INI");
        let controller = AfaController {
            settings_path: path.clone(),
            bindings: AfaBindings::from_ini(&valid_hotkey_values()).expect("bindings"),
            fingerprint,
            process_id: 0,
        };

        std::fs::write(&path, "[Main]\nAutoBeginPause=000\n").expect("change temporary INI");
        assert!(matches!(
            controller.revalidate(),
            Err(AfaError::ConfigChanged)
        ));
        let _ = std::fs::remove_file(path);
    }
}
