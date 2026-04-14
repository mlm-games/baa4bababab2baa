#[cfg(target_arch = "wasm32")]
pub mod wasm;

#[cfg(target_os = "android")]
pub mod android;

#[cfg(all(target_os = "linux", not(target_os = "android")))]
pub mod linux;