use wasodecs::{
    AudioData, AudioEncoded, AudioEncoder, AudioEncoderConfig as WcAudioEncoderConfig,
};

use crate::{
    error::Error,
    traits::{AudioEncoderInput, AudioEncoderOutput},
    types::{AudioDecoderConfig, AudioEncoderConfig, AudioFrame, EncodedAudioPacket},
    util::{samples, validate as v},
};

pub(super) fn to_wc_config(cfg: &AudioEncoderConfig) -> WcAudioEncoderConfig {
    WcAudioEncoderConfig {
        codec: cfg.codec.to_string(),
        channel_count: Some(cfg.channels),
        sample_rate: Some(cfg.sample_rate),
        bitrate: cfg.bitrate,
    }
}

pub struct WasmAudioEncoderInput {
    inner: AudioEncoder,
    config: AudioEncoderConfig,
}

impl AudioEncoderInput for WasmAudioEncoderInput {
    fn encode(&mut self, frame: AudioFrame) -> Result<(), Error> {
        let wc_frame = build_audio_data(&frame)?;
        self.inner
            .encode(&wc_frame)
            .map_err(|e| Error::Platform(format!("{e:?}")))
    }

    async fn flush(&mut self) -> Result<(), Error> {
        self.inner
            .flush()
            .await
            .map_err(|e| Error::Platform(format!("{e:?}")))
    }

    fn queue_size(&self) -> u32 {
        self.inner.queue_size()
    }

    fn config(&self) -> &AudioEncoderConfig {
        &self.config
    }
}

fn build_audio_data(frame: &AudioFrame) -> Result<AudioData, Error> {
    v::audio_frame(frame.channels, frame.sample_rate)?;
    let channels = frame.channels as usize;
    let planar = samples::interleaved_to_planar_f32(&frame.samples, channels, frame.format)
        .map_err(Error::InvalidConfig)?;
    let refs: Vec<&[f32]> = planar.iter().map(|v| v.as_slice()).collect();
    AudioData::new(refs.into_iter(), frame.sample_rate, frame.timestamp)
        .map_err(|e| Error::Platform(format!("{e:?}")))
}

pub struct WasmAudioEncoderOutput {
    inner: AudioEncoded,
    decoder_cfg: Option<AudioDecoderConfig>,
}

impl AudioEncoderOutput for WasmAudioEncoderOutput {
    async fn packet(&mut self) -> Result<Option<EncodedAudioPacket>, Error> {
        let pkt = self.inner.next().await.map_err(|e| match e {
            wasodecs::Error::Dropped => Error::Dropped,
            other => Error::Platform(format!("{other:?}")),
        })?;

        if let Some(wc_cfg) = self.inner.config() {
            self.decoder_cfg = Some(AudioDecoderConfig {
                codec: crate::types::AudioCodecId::from_mime(&wc_cfg.codec),
                channel_count: wc_cfg.channel_count,
                sample_rate: wc_cfg.sample_rate,
                description: wc_cfg.description.clone(),
            });
        }

        Ok(pkt.map(|f| EncodedAudioPacket {
            payload: f.payload,
            timestamp: f.timestamp,
            keyframe: f.keyframe,
        }))
    }

    fn decoder_config(&self) -> Option<&AudioDecoderConfig> {
        self.decoder_cfg.as_ref()
    }
}

pub fn create(
    config: AudioEncoderConfig,
) -> Result<(WasmAudioEncoderInput, WasmAudioEncoderOutput), Error> {
    v::audio_encoder_config(config.channels, config.sample_rate)?;
    let wc_cfg = to_wc_config(&config);
    let (enc, encoded) = wc_cfg
        .init()
        .map_err(|e| Error::Platform(format!("{e:?}")))?;
    Ok((
        WasmAudioEncoderInput { inner: enc, config },
        WasmAudioEncoderOutput {
            inner: encoded,
            decoder_cfg: None,
        },
    ))
}
