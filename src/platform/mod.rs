#[cfg(target_arch = "wasm32")]
pub mod wasm;

#[cfg(target_os = "android")]
pub mod android;

#[cfg(target_os = "linux")]
pub mod linux;