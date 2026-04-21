use cpal::traits::{DeviceTrait, HostTrait};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DeviceError {
    #[error("No audio host available")]
    NoHost,
    #[error("Failed to enumerate devices")]
    EnumerateError,
    #[error("Device not found: {0}")]
    NotFound(String),
}

pub struct AudioDevice {
    pub name: String,
    pub is_default: bool,
    pub sample_rate: u32,
    pub channels: u16,
}

pub fn list_input_devices() -> Result<Vec<AudioDevice>, DeviceError> {
    let host = cpal::default_host();
    let default_in = host.default_input_device();
    let default_name = default_in.as_ref().and_then(|d| d.name().ok());

    let mut devices = Vec::new();
    let cp_devices = host
        .input_devices()
        .map_err(|_| DeviceError::EnumerateError)?;

    for device in cp_devices {
        if let Ok(name) = device.name() {
            let is_default = default_name.as_ref() == Some(&name);

            // Try to gently probe supported configs to get a sense of capability
            let (sample_rate, channels) = if let Ok(mut configs) = device.supported_input_configs()
            {
                if let Some(config) = configs.next() {
                    (config.max_sample_rate().0, config.channels())
                } else {
                    (48000, 1) // Fallback assumptions
                }
            } else {
                (48000, 1)
            };

            devices.push(AudioDevice {
                name,
                is_default,
                sample_rate,
                channels,
            });
        }
    }

    Ok(devices)
}

pub fn get_input_device(name: Option<&str>) -> Result<cpal::Device, DeviceError> {
    let host = cpal::default_host();

    if let Some(target) = name {
        let cp_devices = host
            .input_devices()
            .map_err(|_| DeviceError::EnumerateError)?;
        for device in cp_devices {
            if let Ok(device_name) = device.name() {
                if device_name == target {
                    return Ok(device);
                }
            }
        }
        Err(DeviceError::NotFound(target.to_string()))
    } else {
        host.default_input_device()
            .ok_or(DeviceError::NotFound("Default device".to_string()))
    }
}
