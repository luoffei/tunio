use crate::config::IfaceConfig;
use crate::error::Error;
use crate::platform::wintun::config::WinTunPlatformIfaceConfig;
use crate::platform::wintun::interface::WinTunInterface;
use crate::platform::wintun::logger::wintun_logger;
use crate::traits::DriverT;
use std::sync::Arc;
use wintun_sys;

pub struct WinTunDriver {
    pub wintun: Arc<wintun_sys::wintun>,
}

impl DriverT for WinTunDriver {
    type PlatformInterface = WinTunInterface;
    type PlatformInterfaceConfig = WinTunPlatformIfaceConfig;

    fn new() -> Result<Self, Error> {
        let library_name = "wintun".to_string();
        let wintun = Arc::new(
            unsafe { wintun_sys::wintun::new(library_name) }.map_err(|e| {
                Error::LibraryNotLoaded {
                    reason: format!("{e:?}"),
                }
            })?,
        );

        unsafe {
            wintun.WintunSetLogger(Some(wintun_logger));
        }

        Ok(Self { wintun })
    }

    fn new_interface(
        &mut self,
        config: IfaceConfig<Self>,
    ) -> Result<Self::PlatformInterface, Error> {
        WinTunInterface::new(self.wintun.clone(), config)
    }
}