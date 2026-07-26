// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors
//
// 模板文件本身来自 MaaAssistantArknights (AGPL-3.0-only) 的 resource/template/，
// 派生自《明日方舟》客户端素材。**本仓库不分发这些文件**，运行时从用户配置的
// MAA 资源目录读取。详见仓库根目录 THIRD-PARTY-NOTICES.md。

//! 从用户的 MAA 资源目录加载识别模板。

use std::path::{Path, PathBuf};

use repl_core::geom::Role;

use crate::ncc::{Template, TemplateError};

/// 需要的模板文件名。全都在 MAA 的 `resource/template/` 下（可能在子目录里）。
pub const REQUIRED_TEMPLATES: &[&str] = &[
    "BattleOpersFlag.png",
    "BattleOfficiallyBegin.png",
    "BattleOperRoleCaster.png",
    "BattleOperRoleMedic.png",
    "BattleOperRolePioneer.png",
    "BattleOperRoleSniper.png",
    "BattleOperRoleSpecial.png",
    "BattleOperRoleSupport.png",
    "BattleOperRoleTank.png",
    "BattleOperRoleWarrior.png",
    "BattleOperRoleDrone.png",
];

/// 环境变量：指定 MAA 的 `resource` 目录。
pub const MAA_RESOURCE_ENV: &str = "REPLICATOR_MAA_RESOURCE";

/// 加载好的模板集合。
#[derive(Debug)]
pub struct TemplateSet {
    /// 部署栏卡片顶部的小旗标。用它一次找出所有卡片。
    pub opers_flag: Template,
    /// 战斗 HUD 已经出现的确认标志。
    pub officially_begin: Template,
    /// 九个职业图标。
    pub roles: Vec<(Role, Template)>,
}

impl TemplateSet {
    /// 从 MAA 的 `resource` 目录加载。
    pub fn load(maa_resource_dir: impl AsRef<Path>) -> Result<Self, TemplateLoadError> {
        let dir = maa_resource_dir.as_ref();
        let index = build_index(dir)?;

        let load = |name: &str, mask: Option<(u8, u8)>| -> Result<Template, TemplateLoadError> {
            let path = index
                .iter()
                .find(|p| p.file_name().is_some_and(|f| f == name))
                .ok_or_else(|| TemplateLoadError::Missing(name.to_owned()))?;
            load_png(path, name, mask)
        };

        // maskRange [1,255]：把纯黑的透明背景排除掉（来自 tasks.json 的 BattleOpersFlag）
        let opers_flag = load("BattleOpersFlag.png", Some((1, 255)))?;
        // maskRange [100,255]：来自 tasks.json 的 BattlePauseCancelCheck
        let officially_begin = load("BattleOfficiallyBegin.png", Some((100, 255)))?;

        let roles = [
            (Role::Caster, "BattleOperRoleCaster.png"),
            (Role::Medic, "BattleOperRoleMedic.png"),
            (Role::Pioneer, "BattleOperRolePioneer.png"),
            (Role::Sniper, "BattleOperRoleSniper.png"),
            (Role::Special, "BattleOperRoleSpecial.png"),
            (Role::Support, "BattleOperRoleSupport.png"),
            (Role::Tank, "BattleOperRoleTank.png"),
            (Role::Warrior, "BattleOperRoleWarrior.png"),
            (Role::Drone, "BattleOperRoleDrone.png"),
        ]
        .into_iter()
        .map(|(role, file)| load(file, None).map(|t| (role, t)))
        .collect::<Result<Vec<_>, _>>()?;

        log::info!(
            "loaded {} recognition templates from {}",
            roles.len() + 2,
            dir.display()
        );
        Ok(Self {
            opers_flag,
            officially_begin,
            roles,
        })
    }

    /// 自动定位 MAA 资源目录并加载。
    pub fn load_auto() -> Result<Self, TemplateLoadError> {
        let dir = locate_maa_resource_dir().ok_or(TemplateLoadError::ResourceDirNotFound)?;
        Self::load(dir)
    }
}

/// 自动定位 MAA 的 `resource` 目录。
///
/// 顺序：`REPLICATOR_MAA_RESOURCE` 环境变量 → 当前目录及各级父目录下的
/// `MaaAssistantArknights/resource`。
pub fn locate_maa_resource_dir() -> Option<PathBuf> {
    fn valid(dir: PathBuf) -> Option<PathBuf> {
        dir.join("Arknights-Tile-Pos")
            .join("overview.json")
            .is_file()
            .then_some(dir)
    }
    if let Ok(dir) = std::env::var(MAA_RESOURCE_ENV) {
        if let Some(found) = valid(PathBuf::from(dir)) {
            return Some(found);
        }
    }
    let mut cursor = std::env::current_dir().ok();
    while let Some(dir) = cursor {
        if let Some(found) = valid(dir.join("MaaAssistantArknights").join("resource")) {
            return Some(found);
        }
        cursor = dir.parent().map(Path::to_path_buf);
    }
    None
}

/// 递归列出资源目录下所有 PNG，供按文件名查找。
///
/// MAA 把模板分散在 `template/` 和 `template/Battle/` 等子目录里，
/// 而且不同版本位置会挪，所以按文件名找比写死路径稳。
fn build_index(resource_dir: &Path) -> Result<Vec<PathBuf>, TemplateLoadError> {
    let template_root = resource_dir.join("template");
    if !template_root.is_dir() {
        return Err(TemplateLoadError::ResourceDirInvalid(
            template_root.display().to_string(),
        ));
    }
    let mut out = Vec::new();
    let mut stack = vec![template_root];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("png"))
            {
                out.push(path);
            }
        }
    }
    Ok(out)
}

fn load_png(
    path: &Path,
    name: &str,
    mask: Option<(u8, u8)>,
) -> Result<Template, TemplateLoadError> {
    let image = image::open(path)
        .map_err(|e| TemplateLoadError::Decode(path.display().to_string(), e.to_string()))?
        .to_rgba8();
    let (w, h) = image.dimensions();
    // image crate 给的是 RGBA，NCC 里按 BGRA 读，转一下。
    let mut bgra = Vec::with_capacity((w * h * 4) as usize);
    for px in image.pixels() {
        bgra.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
    }
    Template::from_bgra(name, w, h, &bgra, mask)
        .map_err(|e| TemplateLoadError::Invalid(name.to_owned(), e))
}

#[derive(Debug, thiserror::Error)]
pub enum TemplateLoadError {
    #[error(
        "找不到 MAA 资源目录。请在设置里指定，或设置环境变量 {MAA_RESOURCE_ENV} \
         指向 MaaAssistantArknights 的 resource 目录"
    )]
    ResourceDirNotFound,
    #[error("{0} 不是有效的 MAA 资源目录（缺少 template 子目录）")]
    ResourceDirInvalid(String),
    #[error("MAA 资源目录里找不到模板 {0}")]
    Missing(String),
    #[error("解码 {0} 失败：{1}")]
    Decode(String, String),
    #[error("模板 {0} 不可用：{1}")]
    Invalid(String, #[source] TemplateError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_template_list_is_complete_and_unique() {
        // 1 张旗标 + 1 张开局确认 + 9 个职业
        assert_eq!(REQUIRED_TEMPLATES.len(), 11);
        let mut sorted = REQUIRED_TEMPLATES.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), REQUIRED_TEMPLATES.len(), "列表里有重复项");
    }

    #[test]
    fn missing_resource_dir_is_a_clear_error() {
        let err = TemplateSet::load("Z:/definitely/not/here").unwrap_err();
        assert!(matches!(err, TemplateLoadError::ResourceDirInvalid(_)));
        // 报错信息要能指导用户怎么修
        assert!(err.to_string().contains("MAA"));
    }

    /// 有真实 MAA 资源目录时，全部模板都应当能加载。
    #[test]
    fn loads_real_maa_templates_when_available() {
        let Some(dir) = locate_maa_resource_dir() else {
            eprintln!("跳过：未找到 MAA 资源目录");
            return;
        };
        let set = TemplateSet::load(&dir).expect("加载 MAA 模板失败");
        assert_eq!(set.roles.len(), 9);
        assert!(set.opers_flag.width() > 0 && set.opers_flag.height() > 0);
        // 职业图标尺寸应当和 tasks.json 里的 BattleOperRoleRange (31×25) 量级相当
        for (role, templ) in &set.roles {
            assert!(
                templ.width() <= 40 && templ.height() <= 40,
                "{role:?} 图标尺寸异常：{}×{}",
                templ.width(),
                templ.height()
            );
        }
        eprintln!("已从 {} 加载全部模板", dir.display());
    }
}
