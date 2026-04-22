use std::future::Future;

use crate::error::Error;
use crate::traits::video::MaybeSend;
use crate::types::{AudioDecoderConfig, AudioEncoderConfig, AudioFrame, EncodedAudioPacket};

pub trait AudioEncoderInput: MaybeSend {
    fn encode(&mut self, frame: AudioFrame) -> Result<(), Error>;
    fn flush(&mut self) -> impl Future<Output = Result<(), Error>> + MaybeSend;
    fn queue_size(&self) -> u32;
    fn config(&self) -> &AudioEncoderConfig;
}

pub trait AudioEncoderOutput: MaybeSend {
    fn packet(
        &mut self,
    ) -> impl Future<Output = Result<Option<EncodedAudioPacket>, Error>> + MaybeSend;
    fn decoder_config(&self) -> Option<&AudioDecoderConfig>;
}

pub trait AudioDecoderInput: MaybeSend {
    fn decode(&mut self, packet: EncodedAudioPacket) -> Result<(), Error>;
    fn flush(&mut self) -> impl Future<Output = Result<(), Error>> + MaybeSend;
    fn queue_size(&self) -> u32;
}

pub trait AudioDecoderOutput: MaybeSend {
    fn frame(&mut self) -> impl Future<Output = Result<Option<AudioFrame>, Error>> + MaybeSend;
}
