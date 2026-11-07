//! Windows 侧的 `zlgcan.dll` 绑定。
//!
//! 三个 FFI 面（[D-11] 已核实）：开设备、发、收。其余全是厂商库的内部细节，
//! 本文件只对着厂商头文件的布局填结构体，不做协议处理。
//!
//! **本文件只在 Windows 上编译**，开发机是 Linux，因此这里的内容**未经编译
//! 验证**；真机抽验与 Windows 构建链见 12 号清单与 `docs/90-env.md` 的缺口。
//! 允许在这里放开 `unsafe`：FFI 是本仓唯一必须放开的位置，且收在本模块内，
//! 上层仍 `forbid(unsafe_code)`。
//!
//! [D-11]: ../../../docs/decisions.md#d-11-automotive-后端外挂裁决

#![allow(unsafe_code)]

use std::sync::Mutex;
use std::time::{Duration, Instant};

use openape_bus::{BusError, CanFrame, CanId};
use openape_diag::{Domain, Fault, FaultCode, Severity};

use crate::{BACKEND, ZlgConfig, wire_id};

/// 厂商库的句柄类型。
type Handle = *mut std::ffi::c_void;

/// 打开的设备 + 通道。
#[derive(Debug)]
pub struct Device {
    device: Handle,
    channel: Handle,
    /// 厂商库的收发接口不是线程安全的，收与发必须串行。
    io: Mutex<()>,
}

// 句柄只在 FFI 调用里用；`Bus` 要求 `Send + Sync`，而串行化由 `io` 保证。
unsafe impl Send for Device {}
unsafe impl Sync for Device {}

/// 一帧的厂商表示。字段布局取自厂商头文件 `canfd_frame`。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct CanFdFrame {
    can_id: u32,
    len: u8,
    flags: u8,
    __res0: u8,
    __res1: u8,
    data: [u8; 64],
}

/// `ZCAN_Transmit_Data`。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct TransmitData {
    frame: CanFdFrame,
    transmit_type: u8,
}

/// `ZCAN_Receive_Data`。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct ReceiveData {
    frame: CanFdFrame,
    timestamp: u64,
}

#[link(name = "zlgcan")]
unsafe extern "system" {
    fn ZCAN_OpenDevice(device_type: u32, device_index: u32, reserved: u32) -> Handle;
    fn ZCAN_CloseDevice(device: Handle) -> u32;
    fn ZCAN_InitCAN(device: Handle, channel_index: u32, config: *mut CanFdFrame) -> Handle;
    fn ZCAN_StartCAN(channel: Handle) -> u32;
    fn ZCAN_Transmit(channel: Handle, frames: *const TransmitData, len: u32) -> u32;
    fn ZCAN_Receive(channel: Handle, frames: *mut ReceiveData, len: u32, wait_time: i32) -> u32;
}

impl Device {
    /// 开设备、初始化通道、启动通道。任一步失败都关掉已开的部分。
    pub fn open(channel_name: &str, config: &ZlgConfig) -> Result<Self, Fault> {
        // SAFETY: 句柄在使用期内有效；失败路径负责关闭。
        unsafe {
            let device = ZCAN_OpenDevice(config.device_type, config.device_index, 0);
            if device.is_null() {
                return Err(fault(
                    format!(
                        "{channel_name}: 打不开周立功设备（类型 {} 第 {} 台）",
                        config.device_type, config.device_index
                    ),
                    "确认驱动已装、设备已插好、型号号与设备一致",
                ));
            }
            let mut init = CanFdFrame {
                can_id: config.nominal_bitrate,
                len: 0,
                flags: u8::from(config.can_fd()),
                __res0: 0,
                __res1: 0,
                data: [0; 64],
            };
            let channel = ZCAN_InitCAN(device, config.channel_index, &mut init);
            if channel.is_null() {
                ZCAN_CloseDevice(device);
                return Err(fault(
                    format!(
                        "{channel_name}: 周立功通道 {} 初始化失败",
                        config.channel_index
                    ),
                    "确认波特率与线束一致；FD 需要数据段波特率",
                ));
            }
            if ZCAN_StartCAN(channel) == 0 {
                ZCAN_CloseDevice(device);
                return Err(fault(
                    format!(
                        "{channel_name}: 周立功通道 {} 启动失败",
                        config.channel_index
                    ),
                    "确认设备未被别的进程占用",
                ));
            }
            Ok(Self {
                device,
                channel,
                io: Mutex::new(()),
            })
        }
    }

    pub fn send(&self, frame: &CanFrame) -> Result<(), BusError> {
        let (can_id, extended) =
            wire_id(frame.id).ok_or_else(|| BusError::Transport("标识符超出范围".to_owned()))?;
        let mut vendor = CanFdFrame {
            can_id: if extended {
                can_id | 0x8000_0000
            } else {
                can_id
            },
            len: u8::try_from(frame.data.len()).unwrap_or(0),
            flags: 0,
            __res0: 0,
            __res1: 0,
            data: [0; 64],
        };
        vendor.data[..frame.data.len()].copy_from_slice(&frame.data);
        let transmit = TransmitData {
            frame: vendor,
            transmit_type: 0,
        };
        let _guard = self.io.lock().map_err(|_| BusError::Closed)?;
        // SAFETY: 结构体布局按厂商头文件；句柄在 open 后有效。
        let sent = unsafe { ZCAN_Transmit(self.channel, &transmit, 1) };
        if sent == 0 {
            return Err(BusError::Transport(
                "厂商库拒绝发送（通道忙或离线）".to_owned(),
            ));
        }
        Ok(())
    }

    pub fn recv(&self, timeout: Duration) -> Result<CanFrame, BusError> {
        let wait_ms = i32::try_from(timeout.as_millis()).unwrap_or(i32::MAX);
        let deadline = Instant::now() + timeout;
        loop {
            let mut received = ReceiveData {
                frame: CanFdFrame {
                    can_id: 0,
                    len: 0,
                    flags: 0,
                    __res0: 0,
                    __res1: 0,
                    data: [0; 64],
                },
                timestamp: 0,
            };
            let count = {
                let _guard = self.io.lock().map_err(|_| BusError::Closed)?;
                // SAFETY: 同上；`received` 在本轮内有效。
                unsafe { ZCAN_Receive(self.channel, &mut received, 1, wait_ms) }
            };
            if count > 0 {
                return to_frame(received).ok_or_else(|| {
                    BusError::Transport("厂商库回了长度或标识符非法的帧".to_owned())
                });
            }
            if Instant::now() >= deadline {
                return Err(BusError::Timeout {
                    waited_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
                });
            }
        }
    }
}

fn to_frame(received: ReceiveData) -> Option<CanFrame> {
    let len = usize::from(received.frame.len);
    if len > received.frame.data.len() {
        return None;
    }
    let extended = received.frame.can_id & 0x8000_0000 != 0;
    let raw = received.frame.can_id & 0x1FFF_FFFF;
    let id = if extended {
        CanId::extended(raw)?
    } else {
        CanId::standard(u16::try_from(raw).ok()?)
    };
    CanFrame::new(id, received.frame.data[..len].to_vec())
}

impl Drop for Device {
    fn drop(&mut self) {
        // SAFETY: 句柄在 Drop 里只关一次。
        unsafe {
            ZCAN_CloseDevice(self.device);
        }
    }
}

fn fault(message: String, hint: &str) -> Fault {
    Fault::new(
        FaultCode::new(Domain::Bus, 41).expect("序号 41 合法"),
        Severity::Error,
        message,
    )
    .with_context("backend", BACKEND.to_owned())
    .with_hint(hint)
}
