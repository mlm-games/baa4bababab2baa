use web_codecs::{
    AudioDecoded, AudioDecoder, AudioDecoderConfig as WcAudioDecoderConfig, EncodedFrame,
};

use crate::{
    error::Error,
    traits::{AudioDecoderInput, AudioDecoderOutput},
    types::{AudioDecoderConfig, AudioFrame, EncodedAudioPacket, SampleFormat},
};

fn to_wc_config(cfg: &AudioDecoderConfig) -> WcAudioDecoderConfig {
    let mut wc = WcAudioDecoderConfig::new(&cfg.codec.0, cfg.channel_count, cfg.sample_rate);
    if let Some(desc) = &cfg.description {
        wc.description = Some(desc.clone());
    }
    wc
}

pub struct WasmAudioDecoderInput {
    inner: AudioDecoder,
}

impl AudioDecoderInput for WasmAudioDecoderInput {
    fn decode(&mut self, packet: EncodedAudioPacket) -> Result<(), Error> {
        let frame = EncodedFrame {
            payload: packet.payload,
            timestamp: packet.timestamp,
            keyframe: packet.keyframe,
        };
        self.inner
            .decode(frame)
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
}

pub struct WasmAudioDecoderOutput {
    inner: AudioDecoded,
}

impl AudioDecoderOutput for WasmAudioDecoderOutput {
    async fn frame(&mut self) -> Result<Option<AudioFrame>, Error> {
        let opt = self.inner.next().await.map_err(|e| match e {
            web_codecs::Error::Dropped => Error::Dropped,
            other => Error::Platform(format!("{other:?}")),
        })?;

        let Some(wc_frame) = opt else {
            return Ok(None);
        };

        let channels = wc_frame.number_of_channels();
        let frames = wc_frame.number_of_frames() as usize;
        let sample_rate = wc_frame.sample_rate() as u32;
        let timestamp = wc_frame.timestamp();

        let mut interleaved: Vec<f32> = vec![0.0; channels as usize * frames];

        let mut channel_buf = vec![0.0f32; frames];
        for ch in 0..channels as usize {
            wc_frame
                .copy_to(
                    &mut channel_buf[..],
                    ch,
                    web_codecs::AudioCopyOptions::default(),
                )
                .map_err(|e| Error::Platform(format!("{e:?}")))?;
            for (i, s) in channel_buf.iter().enumerate() {
                interleaved[i * channels as usize + ch] = *s;
            }
        }

        let bytes: Vec<u8> = interleaved.iter().flat_map(|f| f.to_le_bytes()).collect();

        Ok(Some(AudioFrame {
            timestamp,
            sample_rate,
            channels: channels as u32,
            format: SampleFormat::F32,
            samples: bytes,
        }))
    }
}

pub fn create(
    config: AudioDecoderConfig,
) -> Result<(WasmAudioDecoderInput, WasmAudioDecoderOutput), Error> {
    let wc_cfg = to_wc_config(&config);
    let (dec, decoded) = wc_cfg
        .build()
        .map_err(|e| Error::Platform(format!("{e:?}")))?;
    Ok((
        WasmAudioDecoderInput { inner: dec },
        WasmAudioDecoderOutput { inner: decoded },
    ))
}

pub(super) fn to_wc_config_pub(config: &AudioDecoderConfig) -> WcAudioDecoderConfig {
    to_wc_config(config)
}
