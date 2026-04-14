use std::future::Future;

use crate::error::Error;
use crate::types::{AudioDecoderConfig, AudioEncoderConfig, AudioFrame, EncodedAudioPacket};

pub trait AudioEncoderInput: Send {
    fn encode(&mut self, frame: AudioFrame) -> Result<(), Error>;
    fn flush(&mut self) -> impl Future<Output = Result<(), Error>> + Send;
    fn queue_size(&self) -> u32;
    fn config(&self) -> &AudioEncoderConfig;
}

pub trait AudioEncoderOutput: Send {
    fn packet(&mut self) -> impl Future<Output = Result<Option<EncodedAudioPacket>, Error>> + Send;
    fn decoder_config(&self) -> Option<&AudioDecoderConfig>;
}

pub trait AudioDecoderInput: Send {
    fn decode(&mut self, packet: EncodedAudioPacket) -> Result<(), Error>;
    fn flush(&mut self) -> impl Future<Output = Result<(), Error>> + Send;
    fn queue_size(&self) -> u32;
}

pub trait AudioDecoderOutput: Send {
    fn frame(&mut self) -> impl Future<Output = Result<Option<AudioFrame>, Error>> + Send;
}
