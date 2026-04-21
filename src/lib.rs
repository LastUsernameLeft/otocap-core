pub mod devices;
pub mod encoder;
pub mod processor;
pub mod recorder;

pub use encoder::OutputFormat;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessingMode {
    Off,
    Standard,
    Heavy,
}

impl Default for ProcessingMode {
    fn default() -> Self {
        Self::Standard
    }
}

pub struct CaptureOptions {
    pub device_name: Option<String>,
    pub processing_mode: ProcessingMode,
    pub high_pass_filter: bool,
    pub output_format: OutputFormat,
}

impl Default for CaptureOptions {
    fn default() -> Self {
        Self {
            device_name: None,
            processing_mode: ProcessingMode::Standard,
            high_pass_filter: true,
            output_format: OutputFormat::Wav,
        }
    }
}
