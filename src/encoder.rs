use hound::{WavSpec, WavWriter, SampleFormat};
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EncoderError {
    #[error("Failed to create WAV file: {0}")]
    WavError(#[from] hound::Error),
}

pub struct AudioEncoder {
    writer: WavWriter<BufWriter<File>>,
    channels: u16,
}

impl AudioEncoder {
    pub fn new<P: AsRef<Path>>(
        path: P,
        sample_rate: u32,
        channels: u16,
    ) -> Result<Self, EncoderError> {
        let spec = WavSpec {
            channels,
            sample_rate,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };

        let writer = WavWriter::create(path, spec)?;
        
        Ok(Self { writer, channels })
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn test_audio_encoder_creates_wav_file() {
        let test_dir = std::env::temp_dir().join("otocap_tests");
        let _ = fs::create_dir_all(&test_dir);
        let file_path = test_dir.join("test_output.wav");

        // 1. Initialize encoder
        let mut encoder = AudioEncoder::new(&file_path, 48000, 1).expect("Failed to create encoder");

        // 2. Write dummy samples
        let dummy_samples: Vec<i16> = vec![0, 100, 200, 300, 400, 500];
        encoder.write_samples(&dummy_samples).expect("Failed to write samples");

        // 3. Finalize
        encoder.finalize().expect("Failed to finalize WAV file");

        // 4. Verify file exists and has size
        assert!(file_path.exists(), "WAV file was not created");
        let metadata = fs::metadata(&file_path).expect("Failed to read metadata");
        assert!(metadata.len() > 0, "WAV file is empty");

        // Cleanup
        let _ = fs::remove_file(file_path);
    }
}
