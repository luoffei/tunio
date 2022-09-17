#![doc = include_str!("../README.md")]
#![cfg_attr(docsrs, feature(doc_auto_cfg))]
pub mod platform;

pub use tunio_core::config::*;
pub use tunio_core::error::Error;

pub use tunio_core::config;
pub use tunio_core::error;
pub use tunio_core::traits;

cfg_if::cfg_if! {
    if #[cfg(target_os = "windows")] {
        pub type DefaultDriver = platform::wintun::Driver;
        pub type DefaultInterface = platform::wintun::Interface;
        #[cfg(feature = "async-tokio")]
        pub type DefaultTokioInterface = platform::wintun::AsyncTokioInterface;
    }else if #[cfg(target_os = "linux")] {
        pub type DefaultDriver = platform::linux::Driver;
        pub type DefaultInterface = platform::linux::Interface;
        pub type DefaultAsyncInterface = platform::linux::AsyncInterface;
    }else if #[cfg(target_os = "macos")] {
        pub type DefaultDriver = platform::utun::Driver;
        pub type DefaultInterface = platform::utun::Interface;
    }
}