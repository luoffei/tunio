use super::queue::{create_device, Device};
use super::Driver;
use super::PlatformIfConfig;
use delegate::delegate;
use futures::{AsyncRead, AsyncWrite};
use log::debug;
use netconfig::sys::InterfaceHandleExt;
use std::io;
use std::io::{Read, Write};
use std::pin::Pin;
use std::task::{Context, Poll};
use tunio_core::config::IfConfig;
use tunio_core::error::Error;
use tunio_core::file_queue::{AsyncFdQueue, FdQueueT, SyncFdQueue};
use tunio_core::traits::{InterfaceT, SyncQueueT};

pub struct LinuxInterface<Q> {
    name: String,
    pub(crate) queue: Q,
}

impl<Q> LinuxInterface<Q> {
    pub fn name(&self) -> &str {
        &*self.name
    }
}

impl<Q: FdQueueT> InterfaceT for LinuxInterface<Q> {
    type PlatformDriver = Driver;
    type PlatformIfConfig = PlatformIfConfig;

    fn new(
        _driver: &mut Self::PlatformDriver,
        params: IfConfig<Self::PlatformIfConfig>,
    ) -> Result<Self, Error> {
        let Device { device, name } = create_device(&*params.name, params.layer)?;
        let queue = Q::new(device.into());

        if &*params.name != name {
            debug!(
                "Interface name is changed \"{}\" -> \"{}\"",
                &*params.name, name
            );
        }

        Ok(Self { name, queue })
    }

    fn up(&mut self) -> Result<(), Error> {
        Ok(self.handle().set_up(true)?)
    }

    fn down(&mut self) -> Result<(), Error> {
        Ok(self.handle().set_up(false)?)
    }

    fn handle(&self) -> netconfig::InterfaceHandle {
        netconfig::InterfaceHandle::try_from_name(self.name()).unwrap()
    }
}

pub type Interface = LinuxInterface<SyncFdQueue>;

impl SyncQueueT for Interface {}

impl Read for Interface {
    delegate! {
        to self.queue {
            fn read(&mut self, buf: &mut [u8]) -> Result<usize, io::Error>;
        }
    }
}

impl Write for Interface {
    delegate! {
        to self.queue {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize>;
            fn flush(&mut self) -> io::Result<()>;
        }
    }
}

pub type AsyncInterface = LinuxInterface<AsyncFdQueue>;

impl AsyncRead for AsyncInterface {
    delegate! {
        to Pin::new(&mut self.queue) {
            fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>>;
        }
    }
}

impl AsyncWrite for AsyncInterface {
    delegate! {
        to Pin::new(&mut self.queue) {
            fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>>;
            fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>>;
            fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>>;
        }
    }
}