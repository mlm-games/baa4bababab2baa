use std::pin::Pin;
use std::future::Future;

use crate::error::Error;
use crate::traits::video::{FutureMaybeSend, MaybeSend};
use crate::types::{AudioDecoderConfig, AudioEncoderConfig, AudioFrame, EncodedAudioPacket};

/// See [`crate::traits::video::VideoEncoderInput::queue_size`] for
/// semantics of the `queue_size` method.
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

// Boxed companion traits for dyn dispatch

pub trait AudioEncoderInputBoxed: MaybeSend {
    fn encode(&mut self, frame: AudioFrame) -> Result<(), Error>;
    fn flush(&mut self) -> Pin<Box<dyn FutureMaybeSend<Output = Result<(), Error>> + '_>>;
    fn queue_size(&self) -> u32;
    fn config(&self) -> &AudioEncoderConfig;
}

impl<T: AudioEncoderInput + ?Sized> AudioEncoderInputBoxed for T {
    fn encode(&mut self, frame: AudioFrame) -> Result<(), Error> {
        AudioEncoderInput::encode(self, frame)
    }
    fn flush(&mut self) -> Pin<Box<dyn FutureMaybeSend<Output = Result<(), Error>> + '_>> {
        Box::pin(AudioEncoderInput::flush(self))
    }
    fn queue_size(&self) -> u32 {
        AudioEncoderInput::queue_size(self)
    }
    fn config(&self) -> &AudioEncoderConfig {
        AudioEncoderInput::config(self)
    }
}

pub trait AudioEncoderOutputBoxed: MaybeSend {
    fn packet(&mut self) -> Pin<Box<dyn FutureMaybeSend<Output = Result<Option<EncodedAudioPacket>, Error>> + '_>>;
    fn decoder_config(&self) -> Option<&AudioDecoderConfig>;
}

impl<T: AudioEncoderOutput + ?Sized> AudioEncoderOutputBoxed for T {
    fn packet(&mut self) -> Pin<Box<dyn FutureMaybeSend<Output = Result<Option<EncodedAudioPacket>, Error>> + '_>> {
        Box::pin(AudioEncoderOutput::packet(self))
    }
    fn decoder_config(&self) -> Option<&AudioDecoderConfig> {
        AudioEncoderOutput::decoder_config(self)
    }
}

pub trait AudioDecoderInputBoxed: MaybeSend {
    fn decode(&mut self, packet: EncodedAudioPacket) -> Result<(), Error>;
    fn flush(&mut self) -> Pin<Box<dyn FutureMaybeSend<Output = Result<(), Error>> + '_>>;
    fn queue_size(&self) -> u32;
}

impl<T: AudioDecoderInput + ?Sized> AudioDecoderInputBoxed for T {
    fn decode(&mut self, packet: EncodedAudioPacket) -> Result<(), Error> {
        AudioDecoderInput::decode(self, packet)
    }
    fn flush(&mut self) -> Pin<Box<dyn FutureMaybeSend<Output = Result<(), Error>> + '_>> {
        Box::pin(AudioDecoderInput::flush(self))
    }
    fn queue_size(&self) -> u32 {
        AudioDecoderInput::queue_size(self)
    }
}

pub trait AudioDecoderOutputBoxed: MaybeSend {
    fn frame(&mut self) -> Pin<Box<dyn FutureMaybeSend<Output = Result<Option<AudioFrame>, Error>> + '_>>;
}

impl<T: AudioDecoderOutput + ?Sized> AudioDecoderOutputBoxed for T {
    fn frame(&mut self) -> Pin<Box<dyn FutureMaybeSend<Output = Result<Option<AudioFrame>, Error>> + '_>> {
        Box::pin(AudioDecoderOutput::frame(self))
    }
}
