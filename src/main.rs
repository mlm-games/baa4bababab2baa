use baabaabaabaabababbababbaa::{
    Dimensions, VideoCodecId, VideoDecoderConfig, VideoEncoderConfig, default_host,
};

fn main() {
    let host = default_host();

    println!("xcodec: host initialised");

    let dec_config = VideoDecoderConfig {
        codec: VideoCodecId("video/avc".into()),
        resolution: Some(Dimensions::new(1920, 1080)),
        description: None,
        hardware_acceleration: None,
    };

    let enc_config = VideoEncoderConfig {
        codec: VideoCodecId("video/avc".into()),
        dimensions: Dimensions::new(1920, 1080),
        bitrate: Some(4_000_000),
        framerate: Some(30.0),
        hardware_acceleration: None,
        latency_optimized: None,
    };

    println!("Decoder codec:  {}", dec_config.codec.0);
    println!("Encoder codec:  {}", enc_config.codec.0);
    println!(
        "Encoder dims:   {}x{}",
        enc_config.dimensions.width, enc_config.dimensions.height
    );
}
