use crate::{
    error::Error,
    traits::{AudioDecoderInput, AudioDecoderOutput, AudioEncoderInput, AudioEncoderOutput},
    types::{AudioDecoderConfig, AudioEncoderConfig, AudioFrame, EncodedAudioPacket},
};

pub struct CrosAudioEncoderInput {
    pub config: AudioEncoderConfig,
}
pub struct CrosAudioEncoderOutput;
pub struct CrosAudioDecoderInput;
pub struct CrosAudioDecoderOutput;

impl AudioEncoderInput for CrosAudioEncoderInput {
    fn encode(&mut self, _frame: AudioFrame) -> Result<(), Error> {
        Err(Error::Unsupported)
    }

    async fn flush(&mut self) -> Result<(), Error> {
        Err(Error::Unsupported)
    }

    fn queue_size(&self) -> u32 {
        0
    }

    fn config(&self) -> &AudioEncoderConfig {
        &self.config
    }
}

impl AudioEncoderOutput for CrosAudioEncoderOutput {
    async fn packet(&mut self) -> Result<Option<EncodedAudioPacket>, Error> {
        Err(Error::Unsupported)
    }

    fn decoder_config(&self) -> Option<&AudioDecoderConfig> {
        None
    }
}

impl AudioDecoderInput for CrosAudioDecoderInput {
    fn decode(&mut self, _packet: EncodedAudioPacket) -> Result<(), Error> {
        Err(Error::Unsupported)
    }

    async fn flush(&mut self) -> Result<(), Error> {
        Err(Error::Unsupported)
    }

    fn queue_size(&self) -> u32 {
        0
    }
}

impl AudioDecoderOutput for CrosAudioDecoderOutput {
    async fn frame(&mut self) -> Result<Option<AudioFrame>, Error> {
        Err(Error::Unsupported)
    }
}
