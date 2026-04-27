use std::fs::File;
use std::io::{BufWriter, Write};
use std::mem::MaybeUninit;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EncoderError {
    #[error("WAV encoding error: {0}")]
    WavError(#[from] hound::Error),
    #[error("MP3 encoding error: {0}")]
    Mp3Error(String),
    #[error("FLAC encoding error: {0}")]
    FlacError(String),
    #[error("Opus encoding error: {0}")]
    OpusError(String),
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Wav,
    Mp3,
    Opus,
}

impl Default for OutputFormat {
    fn default() -> Self {
        Self::Wav
    }
}

impl OutputFormat {
    pub fn from_extension(path: &Path) -> Self {
        match path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .as_deref()
        {
            Some("mp3") => Self::Mp3,
            Some("opus") | Some("ogg") => Self::Opus,
            _ => Self::Wav,
        }
    }
}

pub enum AudioEncoder {
    Wav(WavEncoder),
    Mp3(Mp3Encoder),
}

impl AudioEncoder {
    pub fn new<P: AsRef<Path>>(
        path: P,
        sample_rate: u32,
        channels: u16,
    ) -> Result<Self, EncoderError> {
        let format = OutputFormat::from_extension(path.as_ref());
        Self::with_format(path, sample_rate, channels, format)
    }

    pub fn with_format<P: AsRef<Path>>(
        path: P,
        sample_rate: u32,
        channels: u16,
        format: OutputFormat,
    ) -> Result<Self, EncoderError> {
        match format {
            OutputFormat::Wav => WavEncoder::new(path, sample_rate, channels).map(Self::Wav),
            OutputFormat::Mp3 => Mp3Encoder::new(path, sample_rate, channels).map(Self::Mp3),
            OutputFormat::Opus => Err(EncoderError::OpusError(
                "Opus container writing requires OGG muxing; use WAV/FLAC/MP3 for now".to_string(),
            )),
        }
    }

    pub fn format(&self) -> OutputFormat {
        match self {
            Self::Wav(_) => OutputFormat::Wav,
            Self::Mp3(_) => OutputFormat::Mp3,
        }
    }

    pub fn write_samples(&mut self, samples: &[i16]) -> Result<(), EncoderError> {
        match self {
            Self::Wav(enc) => enc.write_samples(samples),
            Self::Mp3(enc) => enc.write_samples(samples),
        }
    }

    pub fn finalize(self) -> Result<(), EncoderError> {
        match self {
            Self::Wav(enc) => enc.finalize(),
            Self::Mp3(enc) => enc.finalize(),
        }
    }
}

pub struct WavEncoder {
    writer: hound::WavWriter<BufWriter<File>>,
    _channels: u16,
}

impl WavEncoder {
    pub fn new<P: AsRef<Path>>(
        path: P,
        sample_rate: u32,
        channels: u16,
    ) -> Result<Self, EncoderError> {
        let spec = hound::WavSpec {
            channels,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let writer = hound::WavWriter::create(path, spec)?;
        Ok(Self { writer, _channels: channels })
    }

    pub fn write_samples(&mut self, samples: &[i16]) -> Result<(), EncoderError> {
        for &sample in samples {
            self.writer.write_sample(sample)?;
        }
        Ok(())
    }

    pub fn finalize(self) -> Result<(), EncoderError> {
        self.writer.finalize()?;
        Ok(())
    }
}


pub struct Mp3Encoder {
    encoder: mp3lame_encoder::Encoder,
    writer: BufWriter<File>,
    channels: u16,
    _sample_rate: u32,
}

impl Mp3Encoder {
    pub fn new<P: AsRef<Path>>(
        path: P,
        sample_rate: u32,
        channels: u16,
    ) -> Result<Self, EncoderError> {
        let file = File::create(path)?;
        let writer = BufWriter::new(file);

        let mut builder = mp3lame_encoder::Builder::new()
            .ok_or_else(|| EncoderError::Mp3Error("Failed to create LAME builder".to_string()))?;
        builder
            .set_sample_rate(sample_rate)
            .map_err(|e| EncoderError::Mp3Error(format!("LAME set_sample_rate: {}", e)))?;
        builder
            .set_num_channels(channels as u8)
            .map_err(|e| EncoderError::Mp3Error(format!("LAME set_num_channels: {}", e)))?;
        builder
            .set_brate(mp3lame_encoder::Bitrate::Kbps192)
            .map_err(|e| EncoderError::Mp3Error(format!("LAME set_brate: {}", e)))?;
        builder
            .set_quality(mp3lame_encoder::Quality::Good)
            .map_err(|e| EncoderError::Mp3Error(format!("LAME set_quality: {}", e)))?;

        let encoder = builder
            .build()
            .map_err(|e| EncoderError::Mp3Error(format!("LAME build: {}", e)))?;

        Ok(Self {
            encoder,
            writer,
            channels,
            _sample_rate: sample_rate,
        })
    }

    pub fn write_samples(&mut self, samples: &[i16]) -> Result<(), EncoderError> {
        let ch = self.channels as usize;
        let frame_count = samples.len() / ch;
        if frame_count == 0 {
            return Ok(());
        }

        let buf_size = mp3lame_encoder::max_required_buffer_size(frame_count);
        let mut mp3_buf: Vec<MaybeUninit<u8>> = vec![MaybeUninit::uninit(); buf_size];

        let encoded = if ch == 1 {
            self.encoder
                .encode(mp3lame_encoder::MonoPcm(samples), mp3_buf.as_mut_slice())
                .map_err(|e| EncoderError::Mp3Error(format!("LAME encode: {}", e)))?
        } else {
            let left: Vec<i16> = samples.iter().step_by(ch).copied().collect();
            let right: Vec<i16> = samples.iter().skip(1).step_by(ch).copied().collect();
            self.encoder
                .encode(
                    mp3lame_encoder::DualPcm {
                        left: &left,
                        right: &right,
                    },
                    mp3_buf.as_mut_slice(),
                )
                .map_err(|e| EncoderError::Mp3Error(format!("LAME encode: {}", e)))?
        };

        if encoded > 0 {
            let initialized =
                unsafe { std::slice::from_raw_parts(mp3_buf.as_ptr() as *const u8, encoded) };
            self.writer
                .write_all(initialized)
                .map_err(EncoderError::IoError)?;
        }
        Ok(())
    }

    pub fn finalize(mut self) -> Result<(), EncoderError> {
        let flush_size = mp3lame_encoder::max_required_buffer_size(0);
        let mut mp3_buf: Vec<MaybeUninit<u8>> = vec![MaybeUninit::uninit(); flush_size];

        let encoded = self
            .encoder
            .flush::<mp3lame_encoder::FlushNoGap>(mp3_buf.as_mut_slice())
            .map_err(|e| EncoderError::Mp3Error(format!("LAME flush: {}", e)))?;

        if encoded > 0 {
            let initialized =
                unsafe { std::slice::from_raw_parts(mp3_buf.as_ptr() as *const u8, encoded) };
            self.writer
                .write_all(initialized)
                .map_err(EncoderError::IoError)?;
        }
        self.writer.flush().map_err(EncoderError::IoError)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn test_dir() -> PathBuf {
        let dir = std::env::temp_dir().join("otocap_tests");
        let _ = fs::create_dir_all(&dir);
        dir
    }

    #[test]
    fn test_audio_encoder_creates_wav_file() {
        let file_path = test_dir().join("test_output.wav");
        let mut encoder = AudioEncoder::new(&file_path, 48000, 1).unwrap();
        let dummy_samples: Vec<i16> = vec![0, 100, 200, 300, 400, 500];
        encoder.write_samples(&dummy_samples).unwrap();
        encoder.finalize().unwrap();
        assert!(file_path.exists());
        let metadata = fs::metadata(&file_path).unwrap();
        assert!(metadata.len() > 0);
        let _ = fs::remove_file(file_path);
    }

    #[test]
    fn test_format_detection() {
        assert_eq!(
            OutputFormat::from_extension(Path::new("test.wav")),
            OutputFormat::Wav
        );
        assert_eq!(
            OutputFormat::from_extension(Path::new("test.mp3")),
            OutputFormat::Mp3
        );
        assert_eq!(
            OutputFormat::from_extension(Path::new("test.opus")),
            OutputFormat::Opus
        );
        assert_eq!(
            OutputFormat::from_extension(Path::new("test.ogg")),
            OutputFormat::Opus
        );
        assert_eq!(
            OutputFormat::from_extension(Path::new("test.unknown")),
            OutputFormat::Wav
        );
    }

    #[test]
    fn test_mp3_encoder_creates_file() {
        let file_path = test_dir().join("test_output.mp3");
        let mut encoder =
            AudioEncoder::with_format(&file_path, 48000, 1, OutputFormat::Mp3).unwrap();

        let dummy_samples: Vec<i16> = vec![0; 4800];
        encoder.write_samples(&dummy_samples).unwrap();
        encoder.finalize().unwrap();
        assert!(file_path.exists());
        let metadata = fs::metadata(&file_path).unwrap();
        assert!(metadata.len() > 0);
        let _ = fs::remove_file(file_path);
    }
}
