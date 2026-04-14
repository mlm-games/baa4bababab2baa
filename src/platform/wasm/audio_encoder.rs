use web_codecs::{AudioData, AudioDecoded, AudioEncoded, AudioEncoder, AudioEncoderConfig as WcAudioEncoderConfig};

use crate::{
    error::Error,
    traits::{AudioEncoderInput, AudioEncoderOutput},
    types::{AudioDecoderConfig, AudioEncoderConfig, AudioFrame, EncodedAudioPacket, SampleFormat},
};

fn to_wc_config(cfg: &AudioEncoderConfig) -> WcAudioEncoderConfig {
    WcAudioEncoderConfig {
        codec: cfg.codec.0.clone(),
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
    let channels = frame.channels as usize;
    match frame.format {
        SampleFormat::F32 => {
            let samples: &[f32] = bytemuck_cast_f32(&frame.samples);
            let frames = samples.len() / channels;
            let mut planar: Vec<Vec<f32>> = vec![Vec::with_capacity(frames); channels];
            for (i, s) in samples.iter().enumerate() {
                planar[i % channels].push(*s);
            }
            let refs: Vec<&[f32]> = planar.iter().map(|v| v.as_slice()).collect();
            AudioData::new(refs.into_iter(), frame.sample_rate, frame.timestamp)
                .map_err(|e| Error::Platform(format!("{e:?}")))
        }
        SampleFormat::S16 => {
            let raw: &[i16] = bytemuck_cast_i16(&frame.samples);
            let f32s: Vec<f32> = raw.iter().map(|s| *s as f32 / 32768.0).collect();
            let frames = f32s.len() / channels;
            let mut planar: Vec<Vec<f32>> = vec![Vec::with_capacity(frames); channels];
            for (i, s) in f32s.iter().enumerate() {
                planar[i % channels].push(*s);
            }
            let refs: Vec<&[f32]> = planar.iter().map(|v| v.as_slice()).collect();
            AudioData::new(refs.into_iter(), frame.sample_rate, frame.timestamp)
                .map_err(|e| Error::Platform(format!("{e:?}")))
        }
    }
}

fn bytemuck_cast_f32(bytes: &[u8]) -> &[f32] {
    let (head, body, tail) = unsafe { bytes.align_to::<f32>() };
    assert!(head.is_empty() && tail.is_empty(), "misaligned f32 buffer");
    body
}

fn bytemuck_cast_i16(bytes: &[u8]) -> &[i16] {
    let (head, body, tail) = unsafe { bytes.align_to::<i16>() };
    assert!(head.is_empty() && tail.is_empty(), "misaligned i16 buffer");
    body
}

pub struct WasmAudioEncoderOutput {
    inner: AudioEncoded,
    decoder_cfg: Option<AudioDecoderConfig>,
}

impl AudioEncoderOutput for WasmAudioEncoderOutput {
    async fn packet(&mut self) -> Result<Option<EncodedAudioPacket>, Error> {
        let pkt = self
            .inner
            .frame()
            .await
            .map_err(|e| match e {
                web_codecs::Error::Dropped => Error::Dropped,
                other => Error::Platform(format!("{other:?}")),
            })?;

        if let Some(wc_cfg) = self.inner.config() {
            self.decoder_cfg = Some(AudioDecoderConfig {
                codec: crate::types::AudioCodecId(wc_cfg.codec.clone()),
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

pub(super) fn to_wc_config_pub(config: &AudioEncoderConfig) -> WcAudioEncoderConfig {
    to_wc_config(config)
}