use cpal::traits::{DeviceTrait, HostTrait};
use cpal::{Device, Host};

#[derive(Debug, Clone, PartialEq)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

/// Stable device ID from name (FNV-1a hash). Survives replugs unlike index-based IDs.
fn device_id_from_name(prefix: &str, name: &str) -> String {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET;
    for byte in name.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }

    format!("{}_name:{:016x}", prefix, hash)
}

/// Check for legacy `output_N` / `input_N` IDs.
pub fn is_legacy_index_device_id(device_id: &str) -> bool {
    for prefix in ["output_", "input_"] {
        if let Some(rest) = device_id.strip_prefix(prefix) {
            return rest.parse::<usize>().is_ok();
        }
    }
    false
}

pub struct DeviceManager {
    host: Host,
}

impl DeviceManager {
    pub fn new() -> Self {
        Self {
            host: cpal::default_host(),
        }
    }

    /// Output devices for the panel's device-picker UI (ctl `audio_devices`).
    pub fn get_output_devices(&self) -> Result<Vec<DeviceInfo>, String> {
        crate::always_print!("🔍 [DeviceManager] Starting audio output device enumeration...");
        let mut devices = Vec::new();
        let default_device = self.host.default_output_device();
        let default_name = default_device
            .as_ref()
            .and_then(|d| d.name().ok())
            .unwrap_or_else(|| "Unknown".to_string());

        crate::always_print!("🔍 [DeviceManager] Default device: {}", default_name);

        crate::always_print!("🔍 [DeviceManager] Enumerating output devices via ALSA/cpal...");
        match self.host.output_devices() {
            Ok(device_iter) => {
                for (index, device) in device_iter.enumerate() {
                    if let Ok(name) = device.name() {
                        #[cfg(target_os = "linux")]
                        {
                            // Only virtual mixing nodes are filtered: a USB
                            // headset can expose *only* hw:/plughw:/front:/
                            // surround:/iec958: names, and skipping those hid
                            // the whole device from the picker. dmix:/dsnoop:
                            // are never directly playable via cpal, so they
                            // stay hidden. Every skip is logged so the choice
                            // stays visible in exported logs.
                            if name.starts_with("dmix:") || name.starts_with("dsnoop:") {
                                crate::always_print!(
                                    "🔍 [DeviceManager] Skipping virtual ALSA node: {}",
                                    name
                                );
                                continue;
                            }
                        }

                        let is_default = Some(&name)
                            == default_device.as_ref().and_then(|d| d.name().ok()).as_ref();

                        crate::always_print!(
                            "🔍 [DeviceManager] Found device #{}: {} {}",
                            index,
                            name,
                            if is_default { "(default)" } else { "" }
                        );

                        devices.push(DeviceInfo {
                            id: device_id_from_name("output", &name),
                            name: name.clone(),
                            is_default,
                        });
                    }
                }
                crate::always_print!(
                    "✅ [DeviceManager] Enumeration complete. Found {} devices",
                    devices.len()
                );
            }
            Err(e) => {
                crate::always_print!("❌ [DeviceManager] Failed to enumerate: {}", e);
                return Err(format!("Failed to enumerate output devices: {}", e));
            }
        }

        if devices.is_empty() {
            crate::always_print!("⚠️ [DeviceManager] No devices found, adding default fallback");
            devices.push(DeviceInfo {
                id: "output_default".to_string(),
                name: default_name,
                is_default: true,
            });
        }

        crate::always_print!(
            "🔍 [DeviceManager] Returning {} total devices",
            devices.len()
        );
        Ok(devices)
    }

    pub fn get_output_device_by_id(&self, device_id: &str) -> Result<Option<Device>, String> {
        if device_id == "output_default" {
            return Ok(self.host.default_output_device());
        }

        // Legacy `output_N` ids carry no name, so there is nothing stable to
        // match them against: the Nth device after a replug is usually a
        // different device. Route to the system default (logged) instead of
        // the enumeration index, which played the wrong device.
        if is_legacy_index_device_id(device_id) {
            crate::always_eprint!(
                "⚠️  [DeviceManager] Legacy device id '{}' is index-based and unstable; using system default",
                device_id
            );
            return Ok(self.host.default_output_device());
        }

        match self.host.output_devices() {
            Ok(device_iter) => {
                for device in device_iter {
                    if let Ok(name) = device.name() {
                        if device_id_from_name("output", &name) == device_id {
                            return Ok(Some(device));
                        }
                    }
                }
            }
            Err(e) => {
                return Err(format!("Failed to enumerate devices: {}", e));
            }
        }

        Ok(None)
    }

    /// Name of the current system default output, without a full
    /// enumeration (enumerating activates every ALSA node: 100s of ms —
    /// banned from polling paths; this single query is ~ms and safe on the
    /// commands thread, which the follow-default check in `commands::status`
    /// drives once per panel poll).
    pub fn default_output_name(&self) -> Option<String> {
        self.host.default_output_device()?.name().ok()
    }

    /// Sample rate of the system default output without a full enumeration
    /// (enumerating activates every ALSA device: 100s of ms on the audio
    /// thread). The per-device rate comes from the already-opened `Device`
    /// at the call site instead — see `open_stream` in the audio engine.
    pub fn default_output_sample_rate(&self) -> Option<u32> {
        self.host
            .default_output_device()
            .and_then(|d| d.default_output_config().ok())
            .map(|c| c.sample_rate().0)
    }
}

impl Default for DeviceManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_id_is_stable_for_a_given_name() {
        assert_eq!(
            device_id_from_name("output", "Speakers (Realtek High Definition Audio)"),
            device_id_from_name("output", "Speakers (Realtek High Definition Audio)")
        );
        assert_eq!(
            device_id_from_name("output", "Headphones"),
            "output_name:0b3a976597860826"
        );
    }

    #[test]
    fn device_id_differs_by_name_and_by_direction() {
        assert_ne!(
            device_id_from_name("output", "Headphones"),
            device_id_from_name("output", "Speakers")
        );
        assert_ne!(
            device_id_from_name("output", "Headset"),
            device_id_from_name("input", "Headset")
        );
    }

    #[test]
    fn legacy_index_ids_are_recognised() {
        assert!(is_legacy_index_device_id("output_0"));
        assert!(is_legacy_index_device_id("input_12"));

        assert!(!is_legacy_index_device_id(&device_id_from_name(
            "output",
            "Headphones"
        )));
        assert!(!is_legacy_index_device_id("default"));
        assert!(!is_legacy_index_device_id("output_default"));
    }

    #[test]
    fn default_name_query_agrees_with_enumeration() {
        // The follow-default check calls this on every status poll, so it
        // must never panic (headless CI has no audio: None is fine) and must
        // agree with the enumerated default when both resolve.
        let dm = DeviceManager::new();
        let single = dm.default_output_name();
        if let Some(name) = &single {
            assert!(!name.is_empty());
        }
        if let Ok(devs) = dm.get_output_devices() {
            let enumerated = devs.iter().find(|d| d.is_default).map(|d| &d.name);
            assert_eq!(single.as_ref(), enumerated);
        }
    }
}
