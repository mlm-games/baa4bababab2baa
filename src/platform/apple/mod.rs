//! Apple VideoToolbox backend.
//!
//! * **macOS** (`target_os = "macos"`): real hardware decode/encode via the
//!   runtime-loaded [`oxideav-videotoolbox`] bridge (see [`vt`]). No
//!   compile-time framework link — if VideoToolbox can't load, factories
//!   return [`Error::Unsupported`] so callers fall back to software.
//! * **iOS** (and any other non-macOS target with the `apple` feature):
//!   [`stub`] returning [`Error::Unsupported`]. `oxideav-videotoolbox 0.0.3`
//!   is macOS-only, so iOS stays a stub until upstream gains iOS support.
//!
//! [`oxideav-videotoolbox`]: https://github.com/OxideAV/oxideav-videotoolbox

#[cfg(target_os = "macos")]
mod vt;
#[cfg(target_os = "macos")]
pub use vt::{
    AppleAudioDecoderInput, AppleAudioDecoderOutput, AppleAudioEncoderInput,
    AppleAudioEncoderOutput, AppleVideoDecoderInput, AppleVideoDecoderOutput,
    AppleVideoEncoderInput, AppleVideoEncoderOutput, VideoToolboxHost,
};

#[cfg(not(target_os = "macos"))]
mod stub;
#[cfg(not(target_os = "macos"))]
pub use stub::{
    AppleAudioDecoderInput, AppleAudioDecoderOutput, AppleAudioEncoderInput,
    AppleAudioEncoderOutput, AppleVideoDecoderInput, AppleVideoDecoderOutput,
    AppleVideoEncoderInput, AppleVideoEncoderOutput, VideoToolboxHost,
};
