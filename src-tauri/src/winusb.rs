//! Windows WinUSB 访问层（setupapi + winusb），移植自 `tools/live_block_test/winusb.py`。
//!
//! 设备带 MS OS 描述符，系统自动绑 WinUSB，无需装驱动。
//! 只实现本链路所需的最小面：枚举接口 → 打开 → Bulk OUT/IN → 读字符串描述符取 SN。

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;

use windows_sys::core::GUID;
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW,
    SetupDiGetDeviceInterfaceDetailW, SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W,
    DIGCF_DEVICEINTERFACE, DIGCF_PRESENT,
};
use windows_sys::Win32::Devices::Usb::{
    WinUsb_Free, WinUsb_GetDescriptor, WinUsb_Initialize, WinUsb_QueryInterfaceSettings,
    WinUsb_QueryPipe, WinUsb_ReadPipe, WinUsb_SetPipePolicy, WinUsb_WritePipe,
    USB_INTERFACE_DESCRIPTOR, WINUSB_INTERFACE_HANDLE, WINUSB_PIPE_INFORMATION,
};
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_OVERLAPPED, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};

/// 本设备（0483:66AA）的 IF#0 注册在此 WinUSB 设备接口类下。
pub const GUID_DEVINTERFACE_WINUSB: GUID = GUID {
    data1: 0xDEE824EF,
    data2: 0x729B,
    data3: 0x4A0E,
    data4: [0x9C, 0x14, 0xB7, 0x11, 0x7D, 0x33, 0xA8, 0x17],
};

const PIPE_TRANSFER_TIMEOUT: u32 = 0x03;
const USBD_PIPE_TYPE_BULK: i32 = 2;
const USB_STRING_DESCRIPTOR_TYPE: u8 = 0x03;
const SN_STRING_INDEX: u8 = 3;
const LANGID_EN_US: u16 = 0x0409;
const ERROR_NO_MORE_ITEMS: u32 = 259;
const ERROR_SEM_TIMEOUT: u32 = 121;
const ERROR_OPERATION_ABORTED: u32 = 995;

fn to_wide(s: &str) -> Vec<u16> {
    std::ffi::OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// 枚举 WinUSB 设备接口路径。
pub fn list_paths() -> Result<Vec<String>, String> {
    unsafe {
        let guid = GUID_DEVINTERFACE_WINUSB;
        let hdev = SetupDiGetClassDevsW(
            &guid,
            std::ptr::null(),
            std::ptr::null_mut(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        );
        if hdev == -1 {
            return Err(format!("SetupDiGetClassDevsW 失败：{}", GetLastError()));
        }

        let mut paths = Vec::new();
        let mut idx = 0u32;
        loop {
            let mut did: SP_DEVICE_INTERFACE_DATA = std::mem::zeroed();
            did.cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
            if SetupDiEnumDeviceInterfaces(hdev, std::ptr::null(), &guid, idx, &mut did) == 0 {
                let err = GetLastError();
                if err == ERROR_NO_MORE_ITEMS {
                    break;
                }
                SetupDiDestroyDeviceInfoList(hdev);
                return Err(format!("SetupDiEnumDeviceInterfaces 失败：{err}"));
            }

            let mut need: u32 = 0;
            SetupDiGetDeviceInterfaceDetailW(
                hdev,
                &did,
                std::ptr::null_mut(),
                0,
                &mut need,
                std::ptr::null_mut(),
            );
            if need == 0 {
                idx += 1;
                continue;
            }

            let mut buf = vec![0u8; need as usize];
            // 64 位下 cbSize = 8；DevicePath 紧跟在 u32 之后（偏移 4）
            *(buf.as_mut_ptr() as *mut u32) = 8;
            let ok = SetupDiGetDeviceInterfaceDetailW(
                hdev,
                &did,
                buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W,
                need,
                &mut need,
                std::ptr::null_mut(),
            );
            if ok == 0 {
                let err = GetLastError();
                SetupDiDestroyDeviceInfoList(hdev);
                return Err(format!("SetupDiGetDeviceInterfaceDetailW 失败：{err}"));
            }
            let path_ptr = buf.as_ptr().add(4) as *const u16;
            let mut len = 0usize;
            while *path_ptr.add(len) != 0 {
                len += 1;
            }
            let slice = std::slice::from_raw_parts(path_ptr, len);
            paths.push(String::from_utf16_lossy(slice));
            idx += 1;
        }
        SetupDiDestroyDeviceInfoList(hdev);
        Ok(paths)
    }
}

/// 单个 WinUSB 设备。
pub struct Device {
    file: HANDLE,
    handle: WINUSB_INTERFACE_HANDLE,
    pub pipe_out: u8,
    pub pipe_in: u8,
    pub max_packet: u16,
    pub serial: Option<String>,
}

impl Device {
    pub fn open(path: &str, timeout_ms: u32) -> Result<Device, String> {
        unsafe {
            let wpath = to_wide(path);
            let file = CreateFileW(
                wpath.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null_mut(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                std::ptr::null_mut(),
            );
            if file == INVALID_HANDLE_VALUE || file.is_null() {
                let err = GetLastError();
                return Err(format!(
                    "CreateFileW 失败：{err}（WinError 5 = 设备被占用，GUI 未移交）"
                ));
            }

            let mut handle: WINUSB_INTERFACE_HANDLE = std::ptr::null_mut();
            if WinUsb_Initialize(file, &mut handle) == 0 {
                let err = GetLastError();
                CloseHandle(file);
                return Err(format!("WinUsb_Initialize 失败：{err}"));
            }

            let mut dev = Device {
                file,
                handle,
                pipe_out: 0,
                pipe_in: 0,
                max_packet: 64,
                serial: None,
            };
            dev.query_pipes()?;
            dev.serial = dev.read_serial();
            dev.set_timeout(dev.pipe_out, timeout_ms);
            dev.set_timeout(dev.pipe_in, timeout_ms);
            Ok(dev)
        }
    }

    unsafe fn query_pipes(&mut self) -> Result<(), String> {
        let mut desc: USB_INTERFACE_DESCRIPTOR = unsafe { std::mem::zeroed() };
        if unsafe { WinUsb_QueryInterfaceSettings(self.handle, 0, &mut desc) } == 0 {
            return Err(format!(
                "WinUsb_QueryInterfaceSettings 失败：{}",
                unsafe { GetLastError() }
            ));
        }
        if desc.bInterfaceClass == 0x03 {
            return Err("该接口是 HID，不承载画面（应选 IF#0 厂商 Bulk 接口）".into());
        }
        for i in 0..desc.bNumEndpoints {
            let mut info: WINUSB_PIPE_INFORMATION = unsafe { std::mem::zeroed() };
            if unsafe { WinUsb_QueryPipe(self.handle, 0, i, &mut info) } == 0 {
                continue;
            }
            if info.PipeType == USBD_PIPE_TYPE_BULK {
                if info.PipeId & 0x80 != 0 {
                    self.pipe_in = info.PipeId;
                } else {
                    self.pipe_out = info.PipeId;
                    self.max_packet = if info.MaximumPacketSize == 0 {
                        64
                    } else {
                        info.MaximumPacketSize
                    };
                }
            }
        }
        if self.pipe_out == 0 || self.pipe_in == 0 {
            return Err(format!(
                "未找到 Bulk 端点（OUT={}, IN={}）",
                self.pipe_out, self.pipe_in
            ));
        }
        Ok(())
    }

    fn set_timeout(&mut self, pipe: u8, ms: u32) {
        let val = ms;
        unsafe {
            WinUsb_SetPipePolicy(
                self.handle,
                pipe,
                PIPE_TRANSFER_TIMEOUT,
                4,
                &val as *const u32 as *const c_void,
            );
        }
    }

    unsafe fn read_serial(&self) -> Option<String> {
        let mut buf = [0u8; 0xFF];
        let mut got: u32 = 0;
        let ok = unsafe {
            WinUsb_GetDescriptor(
                self.handle,
                USB_STRING_DESCRIPTOR_TYPE,
                SN_STRING_INDEX,
                LANGID_EN_US,
                buf.as_mut_ptr(),
                0xFF,
                &mut got,
            )
        };
        if ok == 0 || got < 2 {
            return None;
        }
        let raw = &buf[2..got as usize];
        let u16s: Vec<u16> = raw
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        Some(
            String::from_utf16_lossy(&u16s)
                .trim_matches('\0')
                .trim()
                .to_string(),
        )
    }

    /// 写一帧到 Bulk OUT。
    pub fn write(&self, data: &[u8]) -> Result<usize, String> {
        let mut sent: u32 = 0;
        let ok = unsafe {
            WinUsb_WritePipe(
                self.handle,
                self.pipe_out,
                data.as_ptr(),
                data.len() as u32,
                &mut sent,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(format!(
                "WinUsb_WritePipe 失败：{}",
                unsafe { GetLastError() }
            ));
        }
        Ok(sent as usize)
    }

    /// 读回执；超时返回 `Ok(None)`。
    pub fn read(&self, maxlen: usize) -> Result<Option<Vec<u8>>, String> {
        let mut buf = vec![0u8; maxlen];
        let mut got: u32 = 0;
        let ok = unsafe {
            WinUsb_ReadPipe(
                self.handle,
                self.pipe_in,
                buf.as_mut_ptr(),
                maxlen as u32,
                &mut got,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            let err = unsafe { GetLastError() };
            if err == ERROR_SEM_TIMEOUT || err == ERROR_OPERATION_ABORTED {
                return Ok(None);
            }
            return Err(format!("WinUsb_ReadPipe 失败：{err}"));
        }
        buf.truncate(got as usize);
        Ok(Some(buf))
    }
}

// WinUSB 句柄是裸指针，默认非 Send；本链路只在单个任务里串行访问它，故手动标 Send。
unsafe impl Send for Device {}

impl Drop for Device {
    fn drop(&mut self) {
        unsafe {
            if !self.handle.is_null() {
                WinUsb_Free(self.handle);
            }
            if self.file != INVALID_HANDLE_VALUE && !self.file.is_null() {
                CloseHandle(self.file);
            }
        }
    }
}

/// 列出所有 WinUSB 设备的 (path, serial)。
pub fn list_devices() -> Result<Vec<(String, Option<String>)>, String> {
    let mut out = Vec::new();
    for path in list_paths()? {
        match Device::open(&path, 2000) {
            Ok(dev) => out.push((path, dev.serial.clone())),
            Err(_) => out.push((path, None)),
        }
    }
    Ok(out)
}

/// 按 SN 点名打开设备。
pub fn find_by_serial(serial: &str) -> Result<Device, String> {
    let target = serial.trim().to_uppercase();
    let mut last = String::new();
    for path in list_paths()? {
        match Device::open(&path, 3000) {
            Ok(dev) => {
                let sn = dev.serial.clone().unwrap_or_default().to_uppercase();
                if sn == target {
                    return Ok(dev);
                }
                last = format!("SN={sn}");
            }
            Err(e) => last = e,
        }
    }
    Err(format!("未找到 SN={target} 的 WinUSB 设备（{last}）"))
}

/// 打开第一个可用设备（无 SN 时兜底）。
#[allow(dead_code)] // 无 SN 时的兜底入口，暂未接入
pub fn find_first() -> Result<Device, String> {
    let mut last = String::new();
    for path in list_paths()? {
        match Device::open(&path, 3000) {
            Ok(dev) => return Ok(dev),
            Err(e) => last = e,
        }
    }
    Err(format!("未找到可用的 WinUSB 设备（{last}）"))
}
