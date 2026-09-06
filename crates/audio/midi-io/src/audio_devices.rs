//! 音频播放输出设备（CPAL 音频设备）扫描与解析
//!
//! 软件合成器（XSynth / LGS）产生的音频最终经由 CPAL 输出到具体的音频设备
//! （扬声器 / 耳机 / 虚拟音频线缆等）。本模块提供：
//!
//! - [`enumerate_audio_output_devices`]：扫描系统所有可用音频输出设备名；
//! - [`resolve_audio_output_device`]：根据设备名解析出 CPAL `Device`，
//!   设备名无效或缺失时返回 `None`（调用方据此回退到系统默认设备）。
//!
//! 设备名来自 CPAL 枚举，单次运行内唯一可识别，但跨会话不保证稳定
//! （例如设备被重命名 / 重新插拔）。因此配置中仅持久化设备名，
//! 启动时按名重新解析；解析失败则优雅回退到系统默认输出设备。

use cpal::Device;
use cpal::traits::{DeviceTrait, HostTrait};

/// 扫描系统所有可用的音频播放输出设备名称。
///
/// 返回去重后的设备名列表（顺序与 CPAL 枚举一致）。无任何可用设备时返回空列表。
/// 扫描失败（如音频后端不可用）同样返回空列表，不向上抛错。
pub fn enumerate_audio_output_devices() -> Vec<String> {
    let host = cpal::default_host();
    let mut names: Vec<String> = Vec::new();
    if let Ok(devices) = host.output_devices() {
        for device in devices {
            if let Ok(name) = device.name()
                && !names.contains(&name)
            {
                names.push(name);
            }
        }
    }
    tracing::info!("扫描到 {} 个音频播放输出设备", names.len());
    names
}

/// 根据设备名解析出 CPAL 音频输出设备。
///
/// - `None` 或空串：返回 `None`（调用方应使用系统默认输出设备）；
/// - 设备名匹配到某个枚举设备：返回该 `Device`；
/// - 未匹配到（设备已移除 / 改名）：返回 `None`，由调用方回退到系统默认。
///
/// 返回的 `Device` 仅在同一线程内用于打开音频流，不跨线程移动。
pub fn resolve_audio_output_device(name: Option<&str>) -> Option<Device> {
    let target = name?;
    if target.is_empty() {
        return None;
    }

    let host = cpal::default_host();
    let devices = match host.output_devices() {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("枚举音频输出设备失败，回退系统默认: {}", e);
            return None;
        }
    };

    // 单次枚举收集 (设备名, Device)；先按纯逻辑精确匹配设备名（决策是否回退），
    // 命中后再取回对应的 Device 对象。
    let collected: Vec<(String, Device)> = devices
        .filter_map(|d| d.name().ok().map(|n| (n, d)))
        .collect();
    let matched = find_output_device_name(collected.iter().map(|(n, _)| n.as_str()), target)?;
    collected
        .into_iter()
        .find(|(n, _)| n == &matched)
        .map(|(_, d)| d)
}

/// 在设备名集合中按精确匹配查找目标设备名（纯逻辑，与真实 CPAL 枚举解耦，便于测试）。
///
/// - 空目标名 → `None`（表示使用系统默认）；
/// - 命中 → 返回设备名；未命中 → `None`（调用方应回退系统默认）。
///
/// 该决策逻辑被 [`resolve_audio_output_device`] 复用，同时可在单元测试中
/// 用「假设备枚举」验证匹配 / 回退路径，无需依赖真实音频硬件。
pub(crate) fn find_output_device_name(
    mut devices: impl Iterator<Item = impl AsRef<str>>,
    target: &str,
) -> Option<String> {
    if target.is_empty() {
        return None;
    }
    devices
        .find(|n| n.as_ref() == target)
        .map(|n| n.as_ref().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 假设备枚举：用于验证「按名匹配」与「回退」决策，不依赖真实音频硬件。
    fn fake_devices() -> Vec<&'static str> {
        vec!["扬声器 (Realtek)", "耳机 (Bluetooth)", "虚拟音频线缆"]
    }

    #[test]
    fn find_device_matches_exact_name() {
        let devices = fake_devices();
        assert_eq!(
            find_output_device_name(devices.iter().copied(), "耳机 (Bluetooth)"),
            Some("耳机 (Bluetooth)".to_string())
        );
    }

    #[test]
    fn find_device_missing_falls_back_to_default() {
        // 不在枚举中的设备名 → 未命中 → 回退（None）
        let devices = fake_devices();
        assert!(find_output_device_name(devices.iter().copied(), "不存在的设备").is_none());
    }

    #[test]
    fn find_device_empty_target_is_default() {
        // 空目标名 → 使用系统默认（None）
        let devices = fake_devices();
        assert!(find_output_device_name(devices.iter().copied(), "").is_none());
    }

    #[test]
    fn find_device_distinguishes_similar_names() {
        // 名称非前缀匹配：仅精确相等才算命中
        let devices = ["扬声器", "扬声器 (HDMI)"];
        assert!(find_output_device_name(devices.iter().copied(), "扬声器").is_some());
        assert!(find_output_device_name(devices.iter().copied(), "扬声器 (HDMI)").is_some());
        assert!(find_output_device_name(devices.iter().copied(), "扬声器 (USB)").is_none());
    }

    #[test]
    fn resolve_falls_back_when_name_is_none() {
        // None → 系统默认（None），调用方据此打开默认设备
        assert!(resolve_audio_output_device(None).is_none());
    }

    #[test]
    fn resolve_falls_back_when_name_is_empty() {
        assert!(resolve_audio_output_device(Some("")).is_none());
    }

    #[test]
    fn resolve_falls_back_when_device_not_found() {
        // 不存在的设备名 → 解析失败 → 回退（None）。该分支在任意机器上确定可测，
        // 直接验证了「切换失败时回退系统默认」的兜底路径。
        assert!(resolve_audio_output_device(Some("__lumino_no_such_audio_device__")).is_none());
    }

    #[test]
    fn enumerate_returns_deduped_names() {
        // 扫描结果不应包含重复设备名（CPAL 同一 host 下设备名唯一）。
        let names = enumerate_audio_output_devices();
        let mut seen = std::collections::HashSet::new();
        for n in &names {
            assert!(seen.insert(n.clone()), "音频输出设备名重复: {n}");
        }
    }

    #[test]
    fn resolve_matches_real_device_when_present() {
        // 若系统存在音频设备，按扫描到的真实设备名应能解析命中；
        // 无设备（如部分 CI 环境）则跳过，避免误报。
        let devices = enumerate_audio_output_devices();
        if let Some(name) = devices.into_iter().next() {
            assert!(
                resolve_audio_output_device(Some(&name)).is_some(),
                "按真实设备名 '{name}' 解析应命中"
            );
        }
    }
}
