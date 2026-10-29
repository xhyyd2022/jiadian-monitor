//! Windows HID 访问层（hid.dll + setupapi）。
//!
//! 和 `winusb.rs` 分工不同：本模块只服务「外设电量」——枚举 HID 集合、读厂商/产品名、
//! 发 Feature / Output 报告、做带超时的 Input 读。小屏设备走 WinUSB（见 `winusb.rs`），两条路互不相干。
//!
//! 两个关键实现选择：
//!
//! 1. **每个设备开两个句柄**。`HidD_GetFeature` / `HidD_SetFeature` 内部用同步 `DeviceIoControl`，
//!    句柄带 `FILE_FLAG_OVERLAPPED` 时它们会失败；而 `ReadFile` 想要超时又必须用重叠句柄。
//!    所以：`feature` 句柄同步、`io` 句柄重叠，各干各的。
//! 2. **同 VID 常有多个集合**（鼠标 7 字节、键盘 20 字节、厂商私有 64 字节），必须按
//!    `UsagePage / Usage / 各报告长度` 精确区分。这里把端点能力全部暴露给 JSON 的 `match` 判定。

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;

use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW,
    SetupDiGetDeviceInterfaceDetailW, SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W,
    DIGCF_DEVICEINTERFACE, DIGCF_PRESENT,
};
use windows_sys::Win32::Devices::HumanInterfaceDevice::{
    HidD_FreePreparsedData, HidD_GetAttributes, HidD_GetFeature, HidD_GetHidGuid,
    HidD_GetManufacturerString, HidD_GetPreparsedData, HidD_GetProductString,
    HidD_GetSerialNumberString, HidD_SetFeature, HidP_GetCaps, HIDD_ATTRIBUTES, HIDP_CAPS,
    PHIDP_PREPARSED_DATA,
};
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
    ERROR_IO_PENDING,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_FLAG_OVERLAPPED, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

const ERROR_NO_MORE_ITEMS: u32 = 259;
const WAIT_OBJECT_0: u32 = 0;
const WAIT_TIMEOUT: u32 = 258;

/// 一个 HID 集合的静态能力，用于和 JSON 的 `match` 比对。
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct Endpoint {
    pub path: String,
    pub vid: u16,
    pub pid: u16,
    pub version: u16,
    pub usage_page: u16,
    pub usage: u16,
    pub input_len: u16,
    pub output_len: u16,
    pub feature_len: u16,
    pub product: String,
    pub manufacturer: String,
    pub serial: String,
    /// 读写句柄打开失败的原因（系统独占的鼠标/键盘集合、厂商驱动占着私有接口）。
    /// `None` = 正常，能读能写。
    ///
    /// **为什么要留着它**：以前打开失败直接 `continue`，设备在 `--peripherals-hid`
    /// 里凭空消失，用户只看到「什么都没有」，无法判断是自己的鼠标没插、还是被别的软件占了。
    /// 现在改成「仍然列出来 + 带上原因」，能不能读是后面协议层的事。
    pub open_error: Option<String>,
}

fn to_wide(s: &str) -> Vec<u16> {
    std::ffi::OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// 读 HID 字符串描述符（UTF-16LE）。
fn hid_string(file: HANDLE, which: u8) -> String {
    let mut buf = [0u16; 128];
    let len_bytes = (buf.len() * 2) as u32;
    let ok = unsafe {
        match which {
            0 => HidD_GetManufacturerString(file, buf.as_mut_ptr() as *mut c_void, len_bytes),
            1 => HidD_GetProductString(file, buf.as_mut_ptr() as *mut c_void, len_bytes),
            _ => HidD_GetSerialNumberString(file, buf.as_mut_ptr() as *mut c_void, len_bytes),
        }
    };
    if !ok {
        return String::new();
    }
    let len = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len]).trim().to_string()
}

fn open_path(path: &str, overlapped: bool) -> Result<HANDLE, String> {
    let w = to_wide(path);
    let h = unsafe {
        CreateFileW(
            w.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null_mut(),
            OPEN_EXISTING,
            if overlapped { FILE_FLAG_OVERLAPPED } else { 0 },
            std::ptr::null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE || h.is_null() {
        let err = unsafe { GetLastError() };
        return Err(format!(
            "打开 HID 设备失败：{err}（5 = 被独占，例如厂商驱动或游戏软件占着）"
        ));
    }
    Ok(h)
}

/// 只查询、不读写的句柄（`dwDesiredAccess = 0`）。
///
/// 系统独占的鼠标 / 键盘集合用读写方式一定打不开（ERROR_ACCESS_DENIED），但 0 权限能拿到
/// VID:PID / UsagePage / 报告长度 / 产品名 —— 诊断输出够用了。
/// 注意：这种句柄**发不了 Feature/Output 报告**，也读不了 Input，只能用来认设备。
fn open_query(path: &str) -> Option<HANDLE> {
    let w = to_wide(path);
    let h = unsafe {
        CreateFileW(
            w.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null_mut(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE || h.is_null() {
        None
    } else {
        Some(h)
    }
}

fn endpoint_of(path: String, file: HANDLE, open_error: Option<String>) -> Option<Endpoint> {
    unsafe {
        let mut attrs = HIDD_ATTRIBUTES {
            Size: std::mem::size_of::<HIDD_ATTRIBUTES>() as u32,
            VendorID: 0,
            ProductID: 0,
            VersionNumber: 0,
        };
        if !HidD_GetAttributes(file, &mut attrs) {
            return None;
        }
        // 0 权限句柄上 PreparsedData / 字符串描述符可能拿不到：拿不到就退化成
        // 「只有 VID:PID」的端点，也照样列出来，不静默丢掉。
        let mut usage_page = 0u16;
        let mut usage = 0u16;
        let mut input_len = 0u16;
        let mut output_len = 0u16;
        let mut feature_len = 0u16;
        let mut pp: PHIDP_PREPARSED_DATA = 0;
        if HidD_GetPreparsedData(file, &mut pp) && pp != 0 {
            let mut caps = HIDP_CAPS::default();
            let st = HidP_GetCaps(pp, &mut caps);
            HidD_FreePreparsedData(pp);
            if st >= 0 {
                usage_page = caps.UsagePage;
                usage = caps.Usage;
                input_len = caps.InputReportByteLength;
                output_len = caps.OutputReportByteLength;
                feature_len = caps.FeatureReportByteLength;
            }
        }
        Some(Endpoint {
            path,
            vid: attrs.VendorID,
            pid: attrs.ProductID,
            version: attrs.VersionNumber,
            usage_page,
            usage,
            input_len,
            output_len,
            feature_len,
            product: hid_string(file, 1),
            manufacturer: hid_string(file, 0),
            serial: hid_string(file, 2),
            open_error,
        })
    }
}

/// 枚举本机所有 HID 集合。单个集合打开失败只跳过，不影响整体。
pub fn list_endpoints() -> Result<Vec<Endpoint>, String> {
    let mut guid = windows_sys::core::GUID::from_u128(0);
    unsafe { HidD_GetHidGuid(&mut guid) };
    let hdev = unsafe {
        SetupDiGetClassDevsW(
            &guid,
            std::ptr::null(),
            std::ptr::null_mut(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        )
    };
    if hdev == -1 {
        return Err(format!(
            "SetupDiGetClassDevsW(HID) 失败：{}",
            unsafe { GetLastError() }
        ));
    }
    let mut out = Vec::new();
    let mut idx = 0u32;
    loop {
        let mut did: SP_DEVICE_INTERFACE_DATA = unsafe { std::mem::zeroed() };
        did.cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
        if unsafe { SetupDiEnumDeviceInterfaces(hdev, std::ptr::null(), &guid, idx, &mut did) } == 0
        {
            let err = unsafe { GetLastError() };
            if err != ERROR_NO_MORE_ITEMS {
                unsafe { SetupDiDestroyDeviceInfoList(hdev) };
                return Err(format!("SetupDiEnumDeviceInterfaces(HID) 失败：{err}"));
            }
            break;
        }
        idx += 1;

        let mut need: u32 = 0;
        unsafe {
            SetupDiGetDeviceInterfaceDetailW(
                hdev,
                &did,
                std::ptr::null_mut(),
                0,
                &mut need,
                std::ptr::null_mut(),
            )
        };
        if need == 0 {
            continue;
        }
        let mut buf = vec![0u8; need as usize];
        unsafe {
            *(buf.as_mut_ptr() as *mut u32) = 8; // 64 位下 cbSize
        }
        let ok = unsafe {
            SetupDiGetDeviceInterfaceDetailW(
                hdev,
                &did,
                buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W,
                need,
                &mut need,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            continue;
        }
        let path_ptr = unsafe { buf.as_ptr().add(4) as *const u16 };
        let mut len = 0usize;
        while unsafe { *path_ptr.add(len) } != 0 {
            len += 1;
        }
        let path = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(path_ptr, len) });

        match open_path(&path, false) {
            Ok(file) => {
                if let Some(ep) = endpoint_of(path, file, None) {
                    out.push(ep);
                }
                unsafe { CloseHandle(file) };
            }
            Err(e) => {
                // 读写打不开（系统独占的鼠标/键盘集合最常见）：降级成只查询，照样列出来。
                if let Some(file) = open_query(&path) {
                    if let Some(ep) = endpoint_of(path, file, Some(e)) {
                        out.push(ep);
                    }
                    unsafe { CloseHandle(file) };
                }
            }
        }
    }
    unsafe { SetupDiDestroyDeviceInfoList(hdev) };
    Ok(out)
}

/// 打开的 HID 设备（两个句柄：feature 同步、io 重叠）。
#[allow(dead_code)]
pub struct HidDevice {
    feature_file: HANDLE,
    io_file: HANDLE,
    pub path: String,
    pub input_len: usize,
    pub output_len: usize,
    pub feature_len: usize,
}

// HID 句柄是裸指针，默认非 Send。本模块只在一个采集线程里串行使用它。
unsafe impl Send for HidDevice {}

impl HidDevice {
    pub fn open(ep: &Endpoint) -> Result<HidDevice, String> {
        let feature_file = open_path(&ep.path, false)?;
        let io_file = match open_path(&ep.path, true) {
            Ok(h) => h,
            Err(e) => {
                unsafe { CloseHandle(feature_file) };
                return Err(e);
            }
        };
        Ok(HidDevice {
            feature_file,
            io_file,
            path: ep.path.clone(),
            input_len: ep.input_len as usize,
            output_len: ep.output_len as usize,
            feature_len: ep.feature_len as usize,
        })
    }

    /// 读 Feature 报告。`buf` 长度须等于 FeatureReportByteLength。
    pub fn get_feature(&self, buf: &mut [u8]) -> Result<(), String> {
        let ok = unsafe {
            HidD_GetFeature(
                self.feature_file,
                buf.as_mut_ptr() as *mut c_void,
                buf.len() as u32,
            )
        };
        if !ok {
            return Err(format!("HidD_GetFeature 失败：{}", unsafe { GetLastError() }));
        }
        Ok(())
    }

    /// 写 Feature 报告。
    pub fn set_feature(&self, buf: &[u8]) -> Result<(), String> {
        let ok = unsafe {
            HidD_SetFeature(
                self.feature_file,
                buf.as_ptr() as *const c_void,
                buf.len() as u32,
            )
        };
        if !ok {
            return Err(format!("HidD_SetFeature 失败：{}", unsafe { GetLastError() }));
        }
        Ok(())
    }

    /// 写 Output 报告（走中断 OUT 端点）。
    ///
    /// 用 `WriteFile` 而不是 `HidD_SetOutputReport`：后者在部分固件上不触发设备动作，
    /// 而 `WriteFile` 是厂商工具普遍做法。
    pub fn write_output(&self, buf: &[u8]) -> Result<(), String> {
        let ev = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        if ev.is_null() {
            return Err("CreateEventW 失败".into());
        }
        let mut ov = OVERLAPPED::default();
        ov.hEvent = ev;
        let mut sent: u32 = 0;
        let ok = unsafe {
            WriteFile(
                self.io_file,
                buf.as_ptr(),
                buf.len() as u32,
                &mut sent,
                &mut ov,
            )
        };
        let res = if ok != 0 {
            Ok(())
        } else {
            let err = unsafe { GetLastError() };
            if err == ERROR_IO_PENDING {
                let w = unsafe { WaitForSingleObject(ev, 1000) };
                if w == WAIT_OBJECT_0 {
                    let mut n: u32 = 0;
                    if unsafe { GetOverlappedResult(self.io_file, &ov, &mut n, 0) } != 0 {
                        Ok(())
                    } else {
                        Err(format!("WriteFile(HID) 完成失败：{}", unsafe { GetLastError() }))
                    }
                } else {
                    unsafe { CancelIoEx(self.io_file, &ov) };
                    Err("WriteFile(HID) 超时".into())
                }
            } else {
                Err(format!("WriteFile(HID) 失败：{err}"))
            }
        };
        unsafe { CloseHandle(ev) };
        res
    }

    /// 读一条 Input 报告，最多等 `timeout_ms`；没数据返回 `Ok(None)`。
    pub fn read_input(&self, timeout_ms: u32) -> Result<Option<Vec<u8>>, String> {
        let n = self.input_len.max(1);
        let mut buf = vec![0u8; n];
        let ev = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        if ev.is_null() {
            return Err("CreateEventW 失败".into());
        }
        let mut ov = OVERLAPPED::default();
        ov.hEvent = ev;
        let mut got: u32 = 0;
        let ok = unsafe {
            ReadFile(
                self.io_file,
                buf.as_mut_ptr(),
                n as u32,
                &mut got,
                &mut ov,
            )
        };
        let res = if ok != 0 {
            Ok((got as usize, true))
        } else {
            let err = unsafe { GetLastError() };
            if err != ERROR_IO_PENDING {
                Err(format!("ReadFile(HID) 失败：{err}"))
            } else {
                let w = unsafe { WaitForSingleObject(ev, timeout_ms) };
                if w == WAIT_OBJECT_0 {
                    let mut cnt: u32 = 0;
                    if unsafe { GetOverlappedResult(self.io_file, &ov, &mut cnt, 0) } != 0 {
                        Ok((cnt as usize, true))
                    } else {
                        let e2 = unsafe { GetLastError() };
                        // 取消/设备断开都会走到这里，视为「没数据」
                        if e2 == 995 || e2 == 1167 || e2 == 31 {
                            Ok((0, false))
                        } else {
                            Err(format!("ReadFile(HID) 完成失败：{e2}"))
                        }
                    }
                } else if w == WAIT_TIMEOUT {
                    unsafe { CancelIoEx(self.io_file, &ov) };
                    // 必须等取消落地，否则这条 OVERLAPPED 还挂在内核里
                    let mut cnt: u32 = 0;
                    unsafe { GetOverlappedResult(self.io_file, &ov, &mut cnt, 1) };
                    Ok((0, false))
                } else {
                    unsafe { CancelIoEx(self.io_file, &ov) };
                    Err(format!("WaitForSingleObject(HID) 返回 {w}"))
                }
            }
        };
        unsafe { CloseHandle(ev) };
        let (cnt, done) = res?;
        if !done || cnt == 0 {
            return Ok(None);
        }
        buf.truncate(cnt.min(n));
        Ok(Some(buf))
    }
}

impl Drop for HidDevice {
    fn drop(&mut self) {
        unsafe {
            if self.io_file != INVALID_HANDLE_VALUE && !self.io_file.is_null() {
                CancelIoEx(self.io_file, std::ptr::null());
                CloseHandle(self.io_file);
            }
            if self.feature_file != INVALID_HANDLE_VALUE && !self.feature_file.is_null() {
                CloseHandle(self.feature_file);
            }
        }
    }
}
