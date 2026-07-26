// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors
//
// 移植自 arknights-frame-assistant (GPL-3.0-only):
//   src/lib/game_keys.ahk —— 注册表读取、Unity keyId 映射表、默认按键
// 详见仓库根目录 THIRD-PARTY-NOTICES.md。

//! 读取《明日方舟》PC 端的游戏内按键设置。
//!
//! 游戏把键位写在注册表 `HKCU\Software\HyperGryph\Arknights` 下、名字以
//! `KEYBOARD_SETTING_V` 开头的 `REG_BINARY` 值里，内容是一段 UTF-8 JSON，
//! 形如 `{"normalBattle":{"releaseSkill":{"keyId":"alphaE"},...},...}`。
//!
//! 与 AFA 的实现有一处刻意的差异：AFA 用正则去抓 `"xxx":{"keyId":"yyy"}`，
//! 这里改成用 JSON 解析器递归遍历。行为等价，但对格式变动更耐受，也不会被
//! 字符串里的花括号骗到。

use std::collections::BTreeMap;

use crate::keys::KeyCode;

/// 游戏内可绑定的功能。名字就是 JSON 里的字段名。
pub mod func {
    /// 战斗内变速（默认 F）。
    pub const CHANGE_SPEED: &str = "changeSpeed";
    /// 释放技能（默认 E）。
    pub const RELEASE_SKILL: &str = "releaseSkill";
    /// 撤退干员（默认 Q）。
    pub const RETREAT_CHAR: &str = "retreatChar";
    /// 暂停（默认空格）。
    pub const PAUSE_BATTLE: &str = "pauseBattle";
    /// 战斗内左侧弹窗 / 放弃行动（默认 V）。
    pub const BATTLE_LEFT_POPUP: &str = "battleLeftPopup";
}

/// 一套游戏内按键绑定。
#[derive(Clone, Debug)]
pub struct GameKeys {
    bindings: BTreeMap<String, KeyCode>,
    /// 是否成功从注册表读到。为 `false` 时用的是默认值，UI 应当提示用户
    /// 把游戏内按键恢复默认，否则功能可能错位。
    from_registry: bool,
}

impl Default for GameKeys {
    fn default() -> Self {
        Self {
            bindings: default_bindings(),
            from_registry: false,
        }
    }
}

impl GameKeys {
    /// 从注册表加载；失败则回退到默认绑定（并把 `from_registry` 置 false）。
    pub fn load() -> Self {
        match read_registry_json() {
            Ok(Some(json)) => match parse_bindings(&json) {
                Ok(parsed) if !parsed.is_empty() => {
                    let mut bindings = default_bindings();
                    let mut unknown = Vec::new();
                    for (name, key_id) in parsed {
                        match unity_key_id_to_keycode(&key_id) {
                            Some(code) => {
                                bindings.insert(name, code);
                            }
                            None => unknown.push(format!("{name}={key_id}")),
                        }
                    }
                    if !unknown.is_empty() {
                        log::warn!("unrecognised Arknights keyIds (using defaults): {unknown:?}");
                    }
                    log::info!(
                        "loaded {} in-game key bindings from registry",
                        bindings.len()
                    );
                    Self {
                        bindings,
                        from_registry: true,
                    }
                }
                Ok(_) => {
                    log::warn!("Arknights keyboard settings parsed to zero bindings");
                    Self::default()
                }
                Err(e) => {
                    log::warn!("could not parse Arknights keyboard settings: {e}");
                    Self::default()
                }
            },
            Ok(None) => {
                log::warn!("Arknights keyboard settings not found in registry; using defaults");
                Self::default()
            }
            Err(e) => {
                log::warn!("could not read Arknights keyboard settings: {e}");
                Self::default()
            }
        }
    }

    /// 只用默认绑定，不碰注册表（测试用）。
    pub fn defaults_only() -> Self {
        Self::default()
    }

    pub fn from_registry(&self) -> bool {
        self.from_registry
    }

    /// 取某个功能的按键。未知功能返回 `None`。
    pub fn get(&self, function: &str) -> Option<KeyCode> {
        self.bindings.get(function).copied()
    }

    /// 取某个功能的按键，取不到就用给定的兜底值。
    pub fn get_or(&self, function: &str, fallback: KeyCode) -> KeyCode {
        self.get(function).unwrap_or(fallback)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, KeyCode)> {
        self.bindings.iter().map(|(k, v)| (k.as_str(), *v))
    }
}

/// AFA 硬编码的一套默认值，对应游戏的出厂键位。
fn default_bindings() -> BTreeMap<String, KeyCode> {
    let mut m = BTreeMap::new();
    m.insert(func::CHANGE_SPEED.to_owned(), KeyCode(b'F' as u16));
    m.insert(func::RELEASE_SKILL.to_owned(), KeyCode(b'E' as u16));
    m.insert(func::RETREAT_CHAR.to_owned(), KeyCode(b'Q' as u16));
    m.insert(func::PAUSE_BATTLE.to_owned(), KeyCode::SPACE);
    m.insert(func::BATTLE_LEFT_POPUP.to_owned(), KeyCode(b'V' as u16));
    m
}

/// 递归遍历 JSON，收集所有 `{"keyId": "..."}` 形式的叶子对象。
///
/// 返回 `字段名 -> keyId`。游戏把不同场景（常规战斗 / 卫戍协议）的绑定分组放在
/// 不同的父对象里，但字段名本身是全局唯一的，所以扁平收集就够。
fn parse_bindings(json: &str) -> Result<BTreeMap<String, String>, serde_json::Error> {
    let root: serde_json::Value = serde_json::from_str(json)?;
    let mut out = BTreeMap::new();
    collect(&root, &mut out);
    Ok(out)
}

fn collect(value: &serde_json::Value, out: &mut BTreeMap<String, String>) {
    let serde_json::Value::Object(map) = value else {
        return;
    };
    for (name, child) in map {
        if let Some(key_id) = child.get("keyId").and_then(serde_json::Value::as_str) {
            out.insert(name.clone(), key_id.to_owned());
        }
        collect(child, out);
    }
}

/// Unity `keyId` → Windows 虚拟键码。
///
/// 覆盖 AFA `_UnityKeyMap` 的全部条目，外加它那几条模式匹配兜底
/// （`alphaX` / `numX` / 单字符）。符号键映射到**物理键**，与 AFA 一致 ——
/// 例如 `charLeftCurlyBracket`（`{`）对应物理键 `[`。
fn unity_key_id_to_keycode(key_id: &str) -> Option<KeyCode> {
    // 1. 精确匹配（大小写敏感的常见写法）
    if let Some(code) = lookup_exact(key_id) {
        return Some(code);
    }
    // 2. 全小写匹配，兼容不同游戏版本的驼峰 / 全小写变体
    let lower = key_id.to_ascii_lowercase();
    if let Some(code) = lookup_exact(&lower) {
        return Some(code);
    }
    // 3. alphaX -> 字母键
    if let Some(rest) = lower.strip_prefix("alpha") {
        if rest.len() == 1 {
            let c = rest.chars().next()?;
            if c.is_ascii_alphabetic() {
                return KeyCode::from_ascii(c);
            }
            if c.is_ascii_digit() {
                return KeyCode::from_ascii(c);
            }
        }
    }
    // 4. numX -> 主键盘数字键
    if let Some(rest) = lower.strip_prefix("num") {
        if rest.len() == 1 {
            let c = rest.chars().next()?;
            if c.is_ascii_digit() {
                return KeyCode::from_ascii(c);
            }
        }
    }
    // 5. keyFn -> 功能键
    if let Some(rest) = lower.strip_prefix("keyf") {
        if let Ok(n) = rest.parse::<u8>() {
            return KeyCode::function(n);
        }
    }
    if let Some(rest) = lower.strip_prefix('f') {
        if let Ok(n) = rest.parse::<u8>() {
            return KeyCode::function(n);
        }
    }
    // 6. 单字符 keyId，例如 "a" / "4"
    if key_id.chars().count() == 1 {
        return KeyCode::from_ascii(key_id.chars().next()?);
    }
    None
}

fn lookup_exact(key_id: &str) -> Option<KeyCode> {
    // OEM 键的虚拟键码（美式键盘布局）
    const OEM_1: u16 = 0xBA; // ;:
    const OEM_PLUS: u16 = 0xBB; // =+
    const OEM_COMMA: u16 = 0xBC; // ,<
    const OEM_MINUS: u16 = 0xBD; // -_
    const OEM_PERIOD: u16 = 0xBE; // .>
    const OEM_2: u16 = 0xBF; // /?
    const OEM_3: u16 = 0xC0; // `~
    const OEM_4: u16 = 0xDB; // [{
    const OEM_5: u16 = 0xDC; // \|
    const OEM_6: u16 = 0xDD; // ]}
    const OEM_7: u16 = 0xDE; // '"

    let code = match key_id {
        // 特殊功能键
        "keySpace" | "keyspace" | "space" => KeyCode::SPACE,
        "keyTab" | "keytab" | "tab" => KeyCode::TAB,
        "keyEsc" | "keyesc" | "escape" => KeyCode::ESCAPE,
        "keyEnter" | "keyenter" | "keyReturn" | "keyreturn" | "enter" | "return" => KeyCode::ENTER,
        "keyBackspace" | "keybackspace" | "backspace" => KeyCode::BACKSPACE,
        "keyDelete" | "keydelete" | "delete" => KeyCode(0x2E),
        "keyInsert" | "keyinsert" => KeyCode(0x2D),
        "keyHome" | "keyhome" => KeyCode(0x24),
        "keyEnd" | "keyend" => KeyCode(0x23),
        "keyPageUp" | "keypageup" => KeyCode(0x21),
        "keyPageDown" | "keypagedown" => KeyCode(0x22),
        "keyCapsLock" | "keycapslock" => KeyCode(0x14),
        "keyPrint" | "keyprint" => KeyCode(0x2C),
        "keyPause" | "keypause" => KeyCode(0x13),
        "keyScrollLock" | "keyscrolllock" => KeyCode(0x91),
        "keyNumLock" | "keynumlock" => KeyCode(0x90),
        // 修饰键
        "keyShift" | "keyshift" => KeyCode::SHIFT,
        "keyAlt" | "keyalt" => KeyCode::ALT,
        "keyControl" | "keycontrol" => KeyCode::CONTROL,
        "keyLCtrl" | "keylctrl" => KeyCode(0xA2),
        "keyRCtrl" | "keyrctrl" => KeyCode(0xA3),
        "keyLShift" | "keylshift" => KeyCode(0xA0),
        "keyRShift" | "keyrshift" => KeyCode(0xA1),
        "keyLAlt" | "keylalt" => KeyCode(0xA4),
        "keyRAlt" | "keyralt" => KeyCode(0xA5),
        // 方向键
        "keyUp" | "keyup" | "up" => KeyCode(0x26),
        "keyDown" | "keydown" | "down" => KeyCode(0x28),
        "keyLeft" | "keyleft" | "left" => KeyCode(0x25),
        "keyRight" | "keyright" | "right" => KeyCode(0x27),
        // 小键盘
        "keypad0" | "keyAlpha0" => KeyCode(0x60),
        "keypad1" | "keyAlpha1" => KeyCode(0x61),
        "keypad2" | "keyAlpha2" => KeyCode(0x62),
        "keypad3" | "keyAlpha3" => KeyCode(0x63),
        "keypad4" | "keyAlpha4" => KeyCode(0x64),
        "keypad5" | "keyAlpha5" => KeyCode(0x65),
        "keypad6" | "keyAlpha6" => KeyCode(0x66),
        "keypad7" | "keyAlpha7" => KeyCode(0x67),
        "keypad8" | "keyAlpha8" => KeyCode(0x68),
        "keypad9" | "keyAlpha9" => KeyCode(0x69),
        "keypadPeriod" | "keypadperiod" => KeyCode(0x6E),
        "keypadDivide" | "keypaddivide" => KeyCode(0x6F),
        "keypadMultiply" | "keypadmultiply" => KeyCode(0x6A),
        "keypadMinus" | "keypadminus" => KeyCode(0x6D),
        "keypadPlus" | "keypadplus" => KeyCode(0x6B),
        "keypadEnter" | "keypadenter" => KeyCode::ENTER,
        // 符号键 —— 一律映射到物理键，与 AFA 相同
        "charMinus" | "charUnderscore" | "minus" => KeyCode(OEM_MINUS),
        "charPlus" | "charEquals" | "equals" => KeyCode(OEM_PLUS),
        "charPeriod" | "charGreater" | "period" => KeyCode(OEM_PERIOD),
        "charComma" | "charLess" | "comma" => KeyCode(OEM_COMMA),
        "charSlash" | "charQuestion" | "slash" => KeyCode(OEM_2),
        "charBackslash" | "charPipe" | "backslash" => KeyCode(OEM_5),
        "charSemicolon" | "charColon" | "semicolon" => KeyCode(OEM_1),
        "charQuote" | "quote" => KeyCode(OEM_7),
        "charLeftBracket" | "charLeftCurlyBracket" | "charlLeftCurlyBracket" | "leftbracket" => {
            KeyCode(OEM_4)
        }
        "charRightBracket"
        | "charRightCurlyBracket"
        | "charlRightCurlyBracket"
        | "rightbracket" => KeyCode(OEM_6),
        "charBackQuote" | "charTilde" | "backquote" => KeyCode(OEM_3),
        "charExclaim" => KeyCode(b'1' as u16),
        "charAt" => KeyCode(b'2' as u16),
        "charHash" => KeyCode(b'3' as u16),
        "charDollar" => KeyCode(b'4' as u16),
        "charPercent" => KeyCode(b'5' as u16),
        "charCaret" => KeyCode(b'6' as u16),
        "charAmpersand" => KeyCode(b'7' as u16),
        "charAsterisk" => KeyCode(b'8' as u16),
        "charLeftParen" => KeyCode(b'9' as u16),
        "charRightParen" => KeyCode(b'0' as u16),
        _ => return None,
    };
    Some(code)
}

// ————————————————————————————————————————————————————————————————
// 注册表
// ————————————————————————————————————————————————————————————————

/// Unity 的 `PlayerPrefs` 会给键名加 `_h<hash>` 后缀，所以只能按前缀枚举。
const REGISTRY_SUBKEY: &str = r"Software\HyperGryph\Arknights";
const VALUE_PREFIX: &str = "KEYBOARD_SETTING_V";

#[cfg(windows)]
fn read_registry_json() -> Result<Option<String>, String> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{ERROR_NO_MORE_ITEMS, ERROR_SUCCESS, WIN32_ERROR};
    use windows::Win32::System::Registry::{
        RegCloseKey, RegEnumValueW, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER,
        KEY_READ, REG_VALUE_TYPE,
    };

    let subkey = wide(REGISTRY_SUBKEY);
    let mut hkey = HKEY::default();
    // SAFETY: 传入合法的以 NUL 结尾的宽字符串和输出句柄指针。
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            0,
            KEY_READ,
            &mut hkey,
        )
    };
    if status != ERROR_SUCCESS {
        return Ok(None);
    }
    // 从这里往下每条 return 都要 RegCloseKey，用一个小 guard 保证。
    struct KeyGuard(HKEY);
    impl Drop for KeyGuard {
        fn drop(&mut self) {
            // SAFETY: 句柄来自成功的 RegOpenKeyExW。
            unsafe {
                let _ = RegCloseKey(self.0);
            }
        }
    }
    let guard = KeyGuard(hkey);

    // 枚举所有值名，找 KEYBOARD_SETTING_V* 前缀的那个。
    let mut target: Option<Vec<u16>> = None;
    for index in 0.. {
        let mut name = [0u16; 512];
        let mut name_len = name.len() as u32;
        // SAFETY: name/name_len 是配套的缓冲区与长度。
        let status = unsafe {
            RegEnumValueW(
                guard.0,
                index,
                windows::core::PWSTR(name.as_mut_ptr()),
                &mut name_len,
                None,
                None,
                None,
                None,
            )
        };
        if status == WIN32_ERROR(ERROR_NO_MORE_ITEMS.0) {
            break;
        }
        if status != ERROR_SUCCESS {
            return Err(format!("RegEnumValueW failed: {status:?}"));
        }
        let decoded = String::from_utf16_lossy(&name[..name_len as usize]);
        if decoded.starts_with(VALUE_PREFIX) {
            log::debug!("found Arknights keyboard settings value '{decoded}'");
            target = Some(
                name[..name_len as usize]
                    .iter()
                    .copied()
                    .chain([0])
                    .collect(),
            );
            break;
        }
    }
    let Some(value_name) = target else {
        return Ok(None);
    };

    // 先问长度，再读内容。
    let mut kind = REG_VALUE_TYPE::default();
    let mut size = 0u32;
    // SAFETY: 只查询长度，数据指针传 None。
    let status = unsafe {
        RegQueryValueExW(
            guard.0,
            PCWSTR(value_name.as_ptr()),
            None,
            Some(&mut kind),
            None,
            Some(&mut size),
        )
    };
    if status != ERROR_SUCCESS || size == 0 {
        return Ok(None);
    }
    let mut buffer = vec![0u8; size as usize];
    // SAFETY: buffer 至少有 size 字节。
    let status = unsafe {
        RegQueryValueExW(
            guard.0,
            PCWSTR(value_name.as_ptr()),
            None,
            Some(&mut kind),
            Some(buffer.as_mut_ptr()),
            Some(&mut size),
        )
    };
    if status != ERROR_SUCCESS {
        return Err(format!("RegQueryValueExW failed: {status:?}"));
    }
    buffer.truncate(size as usize);
    Ok(Some(decode_reg_binary_utf8(&buffer)))
}

/// 游戏通用设置所在的注册表值名（Unity `PlayerPrefs` 的哈希后缀是固定的）。
const COMMON_SETTING_VALUE: &str = "common_setting_h2012961537";

/// 读取游戏内的 UI 缩放设置（`uiScaler`，取值 0.0–1.0）。
///
/// 这个值决定 HUD 元素离屏幕边缘有多远 —— 部署栏和暂停按钮的位置都受它影响。
/// 默认 1.0（不调整）。读不到时返回 `Ok(None)`，调用方用默认值即可。
pub fn read_ui_scaler() -> Result<Option<f64>, String> {
    read_common_setting_number("uiScaler")
}

/// 读取游戏内的光标大小设置（`cursorSize`）。
pub fn read_cursor_size() -> Result<Option<f64>, String> {
    read_common_setting_number("cursorSize")
}

fn read_common_setting_number(field: &str) -> Result<Option<f64>, String> {
    let Some(json) = read_common_settings()? else {
        return Ok(None);
    };
    let value: serde_json::Value =
        serde_json::from_str(&json).map_err(|e| format!("解析游戏通用设置失败：{e}"))?;
    Ok(value.get(field).and_then(serde_json::Value::as_f64))
}

/// Unity 写 `PlayerPrefs` 字符串时用 `REG_BINARY` 存 UTF-8，并带一个结尾 NUL。
fn decode_reg_binary_utf8(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn read_common_settings() -> Result<Option<String>, String> {
    read_registry_value(COMMON_SETTING_VALUE)
}

/// 按确切的值名读一条 `REG_BINARY` / `REG_SZ`。
#[cfg(windows)]
fn read_registry_value(value_name: &str) -> Result<Option<String>, String> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        RegGetValueW, HKEY_CURRENT_USER, REG_SZ, REG_VALUE_TYPE, RRF_RT_ANY,
    };

    let subkey = wide(REGISTRY_SUBKEY);
    let name = wide(value_name);
    let mut kind = REG_VALUE_TYPE::default();
    let mut size = 0u32;

    // SAFETY: 先查长度，数据指针传 None。
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(name.as_ptr()),
            RRF_RT_ANY,
            Some(&mut kind),
            None,
            Some(&mut size),
        )
    };
    if status != ERROR_SUCCESS || size == 0 {
        return Ok(None);
    }
    let mut buffer = vec![0u8; size as usize];
    // SAFETY: buffer 至少有 size 字节。
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(name.as_ptr()),
            RRF_RT_ANY,
            Some(&mut kind),
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if status != ERROR_SUCCESS {
        return Ok(None);
    }
    buffer.truncate(size as usize);
    if kind == REG_SZ {
        let wide: Vec<u16> = buffer
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let end = wide.iter().position(|c| *c == 0).unwrap_or(wide.len());
        return Ok((end > 0).then(|| String::from_utf16_lossy(&wide[..end])));
    }
    let text = decode_reg_binary_utf8(&buffer);
    Ok((!text.is_empty()).then_some(text))
}

#[cfg(not(windows))]
fn read_registry_json() -> Result<Option<String>, String> {
    Ok(None)
}

#[cfg(not(windows))]
fn read_common_settings() -> Result<Option<String>, String> {
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 游戏实际写进注册表的结构（分组 + keyId 叶子）。
    const SAMPLE: &str = r#"{
        "normalBattle": {
            "changeSpeed":     {"keyId": "alphaF"},
            "releaseSkill":    {"keyId": "alphaE"},
            "retreatChar":     {"keyId": "alphaQ"},
            "pauseBattle":     {"keyId": "keySpace"},
            "battleLeftPopup": {"keyId": "alphaV"}
        },
        "autoChess": {
            "autochessRefresh": {"keyId": "alphaD"},
            "autochessReady":   {"keyId": "alphaC"}
        }
    }"#;

    #[test]
    fn collects_bindings_from_nested_groups() {
        let parsed = parse_bindings(SAMPLE).unwrap();
        assert_eq!(
            parsed.get("releaseSkill").map(String::as_str),
            Some("alphaE")
        );
        assert_eq!(
            parsed.get("pauseBattle").map(String::as_str),
            Some("keySpace")
        );
        assert_eq!(
            parsed.get("autochessRefresh").map(String::as_str),
            Some("alphaD")
        );
        assert_eq!(parsed.len(), 7);
    }

    #[test]
    fn unity_key_ids_map_to_virtual_keys() {
        assert_eq!(unity_key_id_to_keycode("alphaE"), KeyCode::from_ascii('e'));
        assert_eq!(unity_key_id_to_keycode("alphaQ"), KeyCode::from_ascii('q'));
        assert_eq!(unity_key_id_to_keycode("keySpace"), Some(KeyCode::SPACE));
        assert_eq!(unity_key_id_to_keycode("keyEsc"), Some(KeyCode::ESCAPE));
        assert_eq!(unity_key_id_to_keycode("num7"), KeyCode::from_ascii('7'));
        assert_eq!(unity_key_id_to_keycode("keyF5"), KeyCode::function(5));
        assert_eq!(unity_key_id_to_keycode("f5"), KeyCode::function(5));
        // 单字符兜底
        assert_eq!(unity_key_id_to_keycode("a"), KeyCode::from_ascii('a'));
        // 大小写变体
        assert_eq!(unity_key_id_to_keycode("KEYSPACE"), Some(KeyCode::SPACE));
        // { 的物理键是 [
        assert_eq!(
            unity_key_id_to_keycode("charLeftCurlyBracket"),
            unity_key_id_to_keycode("charLeftBracket")
        );
        assert_eq!(unity_key_id_to_keycode("totally_unknown"), None);
    }

    #[test]
    fn defaults_match_afa() {
        let keys = GameKeys::defaults_only();
        assert!(!keys.from_registry());
        assert_eq!(keys.get(func::CHANGE_SPEED), KeyCode::from_ascii('f'));
        assert_eq!(keys.get(func::RELEASE_SKILL), KeyCode::from_ascii('e'));
        assert_eq!(keys.get(func::RETREAT_CHAR), KeyCode::from_ascii('q'));
        assert_eq!(keys.get(func::PAUSE_BATTLE), Some(KeyCode::SPACE));
        assert_eq!(keys.get(func::BATTLE_LEFT_POPUP), KeyCode::from_ascii('v'));
    }

    #[test]
    fn reg_binary_is_utf8_with_trailing_nul() {
        let raw = b"{\"a\":1}\0garbage";
        assert_eq!(decode_reg_binary_utf8(raw), "{\"a\":1}");
        assert_eq!(decode_reg_binary_utf8(b"{}"), "{}");
    }

    #[test]
    fn malformed_json_is_an_error_not_a_panic() {
        assert!(parse_bindings("not json at all").is_err());
        // 合法 JSON 但没有 keyId 叶子 => 空结果，调用方会回退到默认值
        assert!(parse_bindings(r#"{"a":{"b":1}}"#).unwrap().is_empty());
    }
}
