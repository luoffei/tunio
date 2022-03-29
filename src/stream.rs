use bytes::{BufMut, Bytes, BytesMut};
use get_last_error::Win32Error;
use log::warn;
use std::cmp::min;
use std::pin::Pin;
use std::sync::{mpsc, Arc};
use std::task::{Context, Poll, Waker};
use std::thread;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use winapi::shared::winerror::ERROR_NO_MORE_ITEMS;
use winapi::um::handleapi::CloseHandle;

use winapi::um::synchapi::{CreateEventA, SetEvent, WaitForMultipleObjects};
use winapi::um::winbase::{INFINITE, WAIT_OBJECT_0};

use crate::driver::WinTunDriver;
use crate::handle::UnsafeHandle;
use crate::wintun_raw::{DWORD, HANDLE, WINTUN_SESSION_HANDLE};
use crate::WinTunInterface;

enum WorkerCommand {
    Shutdown,
}

pub struct WinTunStream {
    handle: UnsafeHandle<WINTUN_SESSION_HANDLE>,
    driver: Arc<WinTunDriver>,
    #[allow(dead_code)]
    interface: Arc<WinTunInterface>,

    cmd_event: UnsafeHandle<HANDLE>,
    cmd_channel_tx: mpsc::Sender<WorkerCommand>,

    packet_reader_thread: Option<thread::JoinHandle<()>>,
    packet_rx: mpsc::Receiver<Bytes>,

    wakers_tx: mpsc::Sender<Waker>,

    write_status_tx: mpsc::SyncSender<std::io::Result<usize>>,
    write_status_rx: mpsc::Receiver<std::io::Result<usize>>,
    packet_writer_thread: Option<tokio::task::JoinHandle<()>>,
}

const WAIT_OBJECT_1: DWORD = WAIT_OBJECT_0 + 1;

impl WinTunStream {
    pub fn new(
        handle: UnsafeHandle<WINTUN_SESSION_HANDLE>,
        driver: Arc<WinTunDriver>,
        interface: Arc<WinTunInterface>,
    ) -> Self {
        let cmd_event = unsafe { CreateEventA(std::ptr::null_mut(), 0, 0, std::ptr::null_mut()) };
        let (cmd_channel_tx, cmd_channel_rx) = mpsc::channel();

        let inner_handle = UnsafeHandle(handle.0);
        let inner_driver = driver.clone();
        let inner_cmd_event = UnsafeHandle(cmd_event);

        let (packet_tx, packet_rx) = mpsc::sync_channel(16);
        let (wakers_tx, wakers_rx) = mpsc::channel();

        let packet_reader_thread = Some(thread::spawn(|| {
            Self::worker_thread(
                inner_driver,
                inner_handle,
                inner_cmd_event,
                packet_tx,
                cmd_channel_rx,
                wakers_rx,
            )
        }));

        let (write_status_tx, write_status_rx) = mpsc::sync_channel(1);

        WinTunStream {
            handle,
            driver,
            interface,
            cmd_event: UnsafeHandle(cmd_event),
            cmd_channel_tx,
            packet_rx,
            packet_reader_thread,
            wakers_tx,
            write_status_tx,
            write_status_rx,
            packet_writer_thread: None,
        }
    }

    fn worker_thread(
        driver: Arc<WinTunDriver>,
        handle: UnsafeHandle<WINTUN_SESSION_HANDLE>,
        cmd_event: UnsafeHandle<HANDLE>,
        packet_tx: mpsc::SyncSender<Bytes>,
        cmd_channel_rx: mpsc::Receiver<WorkerCommand>,
        wakers_rx: mpsc::Receiver<Waker>,
    ) {
        let read_event = unsafe { driver.wintun.WintunGetReadWaitEvent(handle.0) };
        let mut buffer = BytesMut::new();

        let mut shutdown = false;

        loop {
            if shutdown {
                break;
            }

            let mut packet_len: DWORD = 0;
            let packet = unsafe { driver.wintun.WintunReceivePacket(handle.0, &mut packet_len) };

            if !packet.is_null() {
                unsafe {
                    let packet_slice = std::slice::from_raw_parts(packet, packet_len as usize);
                    buffer.put(packet_slice);
                    driver.wintun.WintunReleaseReceivePacket(handle.0, packet)
                }
                packet_tx
                    .send(buffer.split().freeze())
                    .expect("Stream object is ok");
                while let Ok(waker) = wakers_rx.try_recv() {
                    waker.wake();
                }
            } else {
                let err = Win32Error::get_last_error();
                if err.code() == ERROR_NO_MORE_ITEMS {
                    let event_handles = [cmd_event.0, read_event];
                    let result =
                        unsafe { WaitForMultipleObjects(2, event_handles.as_ptr(), 0, INFINITE) };
                    match result {
                        // Command
                        WAIT_OBJECT_0 => {
                            while let Ok(x) = cmd_channel_rx.try_recv() {
                                match x {
                                    WorkerCommand::Shutdown => {
                                        shutdown = true;
                                    }
                                }
                            }
                        }
                        // Ready for read
                        WAIT_OBJECT_1 => {
                            continue;
                        }
                        e => {
                            panic!("Unexpected event result: {e:?}");
                        }
                    }
                }
            }
        }
        // After loop -- deinitialize
        let _ = unsafe { CloseHandle(cmd_event.0) };
    }

    fn shutdown_worker(&mut self) {
        let _ = self.cmd_channel_tx.send(WorkerCommand::Shutdown);
        unsafe {
            SetEvent(self.cmd_event.0);
        }
        let _ = self.packet_reader_thread.take().unwrap().join();
    }
}

impl Drop for WinTunStream {
    fn drop(&mut self) {
        self.shutdown_worker();
        unsafe {
            self.driver.wintun.WintunEndSession(self.handle.0);
        }
    }
}

impl AsyncRead for WinTunStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if let Ok(buffer) = self.packet_rx.try_recv() {
            let bytes_to_copy = min(buf.capacity() - buf.filled().len(), buffer.len());
            if bytes_to_copy < buffer.len() {
                warn!("Data is truncated: {} > {}", buffer.len(), bytes_to_copy);
            }
            buf.put_slice(&buffer[..bytes_to_copy]);
            Poll::Ready(Ok(()))
        } else {
            let _ = self.wakers_tx.send(cx.waker().clone());
            Poll::Pending
        }
    }
}

impl AsyncWrite for WinTunStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let buffer = Bytes::copy_from_slice(buf);

        let inner_handle = UnsafeHandle(self.handle.0);
        let inner_driver = self.driver.clone();
        let inner_write_status_tx = self.write_status_tx.clone();
        let waker = cx.waker().clone();

        if let Ok(result) = self.write_status_rx.try_recv() {
            Poll::Ready(result)
        } else {
            self.get_mut().packet_writer_thread = Some(tokio::task::spawn_blocking(move || {
                let inner_handle = inner_handle;

                let packet = unsafe {
                    inner_driver
                        .wintun
                        .WintunAllocateSendPacket(inner_handle.0, buffer.len() as DWORD)
                };

                // Copy buffer to allocated packet
                unsafe {
                    packet.copy_from_nonoverlapping(buffer.as_ptr(), buffer.len());
                }

                unsafe {
                    inner_driver.wintun.WintunSendPacket(inner_handle.0, packet);
                }

                let _ = inner_write_status_tx.send(Ok(buffer.len()));
                waker.wake();
            }));
            Poll::Pending
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Poll::Ready(Ok(()))
    }
}
