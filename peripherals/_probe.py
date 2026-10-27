"""探测脚本：直接用 ctypes 打开 HID 端点，试各种 feature/output 读法，找出真实电量。

不参与构建、不进部署包（文件名以下划线开头，build.ps1 会跳过）。
用法：
    python _probe.py list
    python _probe.py get <vid:pid> <usagePage> <usage> <hex 请求帧>
    python _probe.py set <vid:pid> <usagePage> <usage> <hex 请求帧>
    python _probe.py read <vid:pid> <usagePage> <usage> [超时ms]
"""

import ctypes
import sys
from ctypes import wintypes

hid = ctypes.WinDLL("hid.dll")
setupapi = ctypes.WinDLL("setupapi.dll")
kernel32 = ctypes.WinDLL("kernel32.dll")

DIGCF_PRESENT = 0x02
DIGCF_DEVICEINTERFACE = 0x10
GENERIC_READ = 0x80000000
GENERIC_WRITE = 0x40000000
FILE_SHARE_READ = 0x01
FILE_SHARE_WRITE = 0x02
OPEN_EXISTING = 3
INVALID_HANDLE_VALUE = ctypes.c_void_p(-1).value


class GUID(ctypes.Structure):
    _fields_ = [
        ("Data1", wintypes.DWORD),
        ("Data2", wintypes.WORD),
        ("Data3", wintypes.WORD),
        ("Data4", ctypes.c_ubyte * 8),
    ]


class SP_DEVICE_INTERFACE_DATA(ctypes.Structure):
    _fields_ = [
        ("cbSize", wintypes.DWORD),
        ("InterfaceClassGuid", GUID),
        ("Flags", wintypes.DWORD),
        ("Reserved", ctypes.POINTER(ctypes.c_void_p)),
    ]


class HIDD_ATTRIBUTES(ctypes.Structure):
    _fields_ = [
        ("Size", wintypes.ULONG),
        ("VendorID", ctypes.c_ushort),
        ("ProductID", ctypes.c_ushort),
        ("VersionNumber", ctypes.c_ushort),
    ]


class HIDP_CAPS(ctypes.Structure):
    _fields_ = [
        ("Usage", ctypes.c_ushort),
        ("UsagePage", ctypes.c_ushort),
        ("InputReportByteLength", ctypes.c_ushort),
        ("OutputReportByteLength", ctypes.c_ushort),
        ("FeatureReportByteLength", ctypes.c_ushort),
        ("Reserved", ctypes.c_ushort * 17),
        ("NumberLinkCollectionNodes", ctypes.c_ushort),
        ("NumberInputButtonCaps", ctypes.c_ushort),
        ("NumberInputValueCaps", ctypes.c_ushort),
        ("NumberInputDataIndices", ctypes.c_ushort),
        ("NumberOutputButtonCaps", ctypes.c_ushort),
        ("NumberOutputValueCaps", ctypes.c_ushort),
        ("NumberOutputDataIndices", ctypes.c_ushort),
        ("NumberFeatureButtonCaps", ctypes.c_ushort),
        ("NumberFeatureValueCaps", ctypes.c_ushort),
        ("NumberFeatureDataIndices", ctypes.c_ushort),
    ]


hid.HidD_GetHidGuid.argtypes = [ctypes.POINTER(GUID)]
hid.HidD_GetAttributes.argtypes = [wintypes.HANDLE, ctypes.POINTER(HIDD_ATTRIBUTES)]
hid.HidD_GetAttributes.restype = wintypes.BOOLEAN
hid.HidD_GetPreparsedData.argtypes = [wintypes.HANDLE, ctypes.POINTER(ctypes.c_void_p)]
hid.HidD_GetPreparsedData.restype = wintypes.BOOLEAN
hid.HidD_FreePreparsedData.argtypes = [ctypes.c_void_p]
hid.HidP_GetCaps.argtypes = [ctypes.c_void_p, ctypes.POINTER(HIDP_CAPS)]
hid.HidP_GetCaps.restype = ctypes.c_long
hid.HidD_GetProductString.argtypes = [wintypes.HANDLE, ctypes.c_void_p, wintypes.ULONG]
hid.HidD_GetProductString.restype = wintypes.BOOLEAN
hid.HidD_GetFeature.argtypes = [wintypes.HANDLE, ctypes.c_void_p, wintypes.ULONG]
hid.HidD_GetFeature.restype = wintypes.BOOLEAN
hid.HidD_SetFeature.argtypes = [wintypes.HANDLE, ctypes.c_void_p, wintypes.ULONG]
hid.HidD_SetFeature.restype = wintypes.BOOLEAN
hid.HidD_SetOutputReport.argtypes = [wintypes.HANDLE, ctypes.c_void_p, wintypes.ULONG]
hid.HidD_SetOutputReport.restype = wintypes.BOOLEAN

setupapi.SetupDiGetClassDevsW.argtypes = [
    ctypes.POINTER(GUID),
    wintypes.LPCWSTR,
    wintypes.HWND,
    wintypes.DWORD,
]
setupapi.SetupDiGetClassDevsW.restype = wintypes.HANDLE
setupapi.SetupDiEnumDeviceInterfaces.argtypes = [
    wintypes.HANDLE,
    ctypes.c_void_p,
    ctypes.POINTER(GUID),
    wintypes.DWORD,
    ctypes.POINTER(SP_DEVICE_INTERFACE_DATA),
]
setupapi.SetupDiEnumDeviceInterfaces.restype = wintypes.BOOL
setupapi.SetupDiGetDeviceInterfaceDetailW.argtypes = [
    wintypes.HANDLE,
    ctypes.POINTER(SP_DEVICE_INTERFACE_DATA),
    ctypes.c_void_p,
    wintypes.DWORD,
    ctypes.POINTER(wintypes.DWORD),
    ctypes.c_void_p,
]
setupapi.SetupDiGetDeviceInterfaceDetailW.restype = wintypes.BOOL
setupapi.SetupDiDestroyDeviceInfoList.argtypes = [wintypes.HANDLE]

kernel32.CreateFileW.argtypes = [
    wintypes.LPCWSTR,
    wintypes.DWORD,
    wintypes.DWORD,
    ctypes.c_void_p,
    wintypes.DWORD,
    wintypes.DWORD,
    wintypes.HANDLE,
]
kernel32.CreateFileW.restype = wintypes.HANDLE
kernel32.CloseHandle.argtypes = [wintypes.HANDLE]


def endpoints():
    guid = GUID()
    hid.HidD_GetHidGuid(ctypes.byref(guid))
    hdev = setupapi.SetupDiGetClassDevsW(
        ctypes.byref(guid), None, None, DIGCF_PRESENT | DIGCF_DEVICEINTERFACE
    )
    out = []
    i = 0
    while True:
        did = SP_DEVICE_INTERFACE_DATA()
        did.cbSize = ctypes.sizeof(did)
        if not setupapi.SetupDiEnumDeviceInterfaces(
            hdev, None, ctypes.byref(guid), i, ctypes.byref(did)
        ):
            break
        need = wintypes.DWORD()
        setupapi.SetupDiGetDeviceInterfaceDetailW(
            hdev, ctypes.byref(did), None, 0, ctypes.byref(need), None
        )
        buf = ctypes.create_string_buffer(need.value)
        ctypes.cast(buf, ctypes.POINTER(wintypes.DWORD))[0] = 8
        if setupapi.SetupDiGetDeviceInterfaceDetailW(
            hdev, ctypes.byref(did), buf, need.value, ctypes.byref(need), None
        ):
            path = ctypes.wstring_at(ctypes.addressof(buf) + 4)
            out.append(path)
        i += 1
    setupapi.SetupDiDestroyDeviceInfoList(hdev)
    return out


def describe(path):
    h = kernel32.CreateFileW(
        path, 0, FILE_SHARE_READ | FILE_SHARE_WRITE, None, OPEN_EXISTING, 0, None
    )
    if h == INVALID_HANDLE_VALUE:
        return None
    try:
        attrs = HIDD_ATTRIBUTES()
        attrs.Size = ctypes.sizeof(attrs)
        if not hid.HidD_GetAttributes(h, ctypes.byref(attrs)):
            return None
        pp = ctypes.c_void_p()
        caps = HIDP_CAPS()
        if not hid.HidD_GetPreparsedData(h, ctypes.byref(pp)):
            return None
        hid.HidP_GetCaps(pp, ctypes.byref(caps))
        hid.HidD_FreePreparsedData(pp)
        pb = ctypes.create_unicode_buffer(128)
        hid.HidD_GetProductString(h, pb, ctypes.sizeof(pb))
        return {
            "path": path,
            "vid": attrs.VendorID,
            "pid": attrs.ProductID,
            "usage_page": caps.UsagePage,
            "usage": caps.Usage,
            "in_len": caps.InputReportByteLength,
            "out_len": caps.OutputReportByteLength,
            "feat_len": caps.FeatureReportByteLength,
            "product": pb.value,
        }
    finally:
        kernel32.CloseHandle(h)


def open_path(path):
    return kernel32.CreateFileW(
        path,
        GENERIC_READ | GENERIC_WRITE,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
        None,
        OPEN_EXISTING,
        0,
        None,
    )


def pick(vidpid, usage_page=None, usage=None, in_len=None):
    want = vidpid.upper()
    for path in endpoints():
        d = describe(path)
        if not d:
            continue
        if f"{d['vid']:04X}:{d['pid']:04X}" != want:
            continue
        if usage_page is not None and d["usage_page"] != usage_page:
            continue
        if usage is not None and d["usage"] != usage:
            continue
        if in_len is not None and d["in_len"] != in_len:
            continue
        return d
    return None


def hexbytes(s):
    s = s.replace(",", " ")
    return bytes(int(x, 16) for x in s.split())


def cmd_list():
    for path in endpoints():
        d = describe(path)
        if not d:
            continue
        print(
            f"{d['vid']:04X}:{d['pid']:04X} up={d['usage_page']:04X} u={d['usage']:04X} "
            f"in={d['in_len']} out={d['out_len']} feat={d['feat_len']} {d['product']}"
        )


def run(mode, vidpid, up, us, payload=None):
    d = pick(vidpid, up, us)
    if not d:
        print("找不到该端点")
        return 1
    print(
        f"打开 {d['vid']:04X}:{d['pid']:04X} up={d['usage_page']:04X} u={d['usage']:04X} "
        f"in={d['in_len']} out={d['out_len']} feat={d['feat_len']} {d['product']}"
    )
    h = open_path(d["path"])
    if h == INVALID_HANDLE_VALUE:
        print("打开失败")
        return 1
    try:
        if mode == "get":
            buf = ctypes.create_string_buffer(payload, d["feat_len"] or len(payload))
            ok = hid.HidD_GetFeature(h, buf, d["feat_len"] or len(payload))
            print("GetFeature ok=", bool(ok))
            if ok:
                print("  <-", buf.raw.hex(" "))
                show_candidates(buf.raw)
        elif mode == "set":
            buf = ctypes.create_string_buffer(payload, d["feat_len"] or len(payload))
            ok = hid.HidD_SetFeature(h, buf, d["feat_len"] or len(payload))
            print("SetFeature ok=", bool(ok))
            buf2 = ctypes.create_string_buffer(d["feat_len"] or len(payload))
            ok2 = hid.HidD_GetFeature(h, buf2, d["feat_len"] or len(payload))
            print("GetFeature ok=", bool(ok2))
            if ok2:
                print("  <-", buf2.raw.hex(" "))
                show_candidates(buf2.raw)
        elif mode == "setout":
            buf = ctypes.create_string_buffer(payload, d["out_len"] or len(payload))
            ok = hid.HidD_SetOutputReport(h, buf, d["out_len"] or len(payload))
            print("SetOutputReport ok=", bool(ok))
    finally:
        kernel32.CloseHandle(h)
    return 0


def xfer(vidpid, up, us, payload_hex, in_len=None, seconds=1.0):
    """WriteFile 发 output report，然后 ReadFile 收集输入报告（带超时）。"""
    import time

    d = pick(vidpid, up, us, in_len)
    if not d:
        print("找不到该端点")
        return 1
    n = d["in_len"] or 64
    out = bytes(hexbytes(payload_hex))
    h = kernel32.CreateFileW(
        d["path"],
        GENERIC_READ | GENERIC_WRITE,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
        None,
        OPEN_EXISTING,
        FILE_FLAG_OVERLAPPED,
        None,
    )
    if h == INVALID_HANDLE_VALUE:
        print("打开失败")
        return 1
    print(f"发 {len(out)} 字节 -> up={d['usage_page']:04X} u={d['usage']:04X}: {out.hex(' ')}")
    try:
        # 先把残留输入全部读掉
        while True:
            ev = kernel32.CreateEventW(None, True, False, None)
            ov = OVERLAPPED()
            ov.hEvent = ev
            buf = ctypes.create_string_buffer(n)
            got = wintypes.DWORD()
            ok = kernel32.ReadFile(h, buf, n, ctypes.byref(got), ctypes.byref(ov))
            if not ok:
                if kernel32.WaitForSingleObject(ev, 0) != WAIT_OBJECT_0:
                    kernel32.CancelIoEx(h, ctypes.byref(ov))
                    kernel32.GetOverlappedResult(h, ctypes.byref(ov), ctypes.byref(got), True)
                    kernel32.CloseHandle(ev)
                    break
                kernel32.GetOverlappedResult(h, ctypes.byref(ov), ctypes.byref(got), False)
            kernel32.CloseHandle(ev)
            if not got.value:
                break

        written = wintypes.DWORD()
        ev2 = kernel32.CreateEventW(None, True, False, None)
        ov2 = OVERLAPPED()
        ov2.hEvent = ev2
        wbuf = ctypes.create_string_buffer(out, len(out))
        ok = kernel32.WriteFile(h, wbuf, len(out), ctypes.byref(written), ctypes.byref(ov2))
        if not ok:
            if kernel32.WaitForSingleObject(ev2, 500) == WAIT_OBJECT_0:
                kernel32.GetOverlappedResult(h, ctypes.byref(ov2), ctypes.byref(written), False)
            else:
                kernel32.CancelIoEx(h, ctypes.byref(ov2))
                kernel32.GetOverlappedResult(h, ctypes.byref(ov2), ctypes.byref(written), True)
        kernel32.CloseHandle(ev2)
        print("WriteFile 写入字节数 =", written.value)

        end = time.time() + seconds
        seen = 0
        while time.time() < end:
            ev = kernel32.CreateEventW(None, True, False, None)
            ov = OVERLAPPED()
            ov.hEvent = ev
            buf = ctypes.create_string_buffer(n)
            got = wintypes.DWORD()
            ok = kernel32.ReadFile(h, buf, n, ctypes.byref(got), ctypes.byref(ov))
            if not ok:
                if kernel32.WaitForSingleObject(ev, 300) != WAIT_OBJECT_0:
                    kernel32.CancelIoEx(h, ctypes.byref(ov))
                    kernel32.GetOverlappedResult(h, ctypes.byref(ov), ctypes.byref(got), True)
                    kernel32.CloseHandle(ev)
                    continue
                kernel32.GetOverlappedResult(h, ctypes.byref(ov), ctypes.byref(got), False)
            kernel32.CloseHandle(ev)
            if got.value:
                seen += 1
                print(f"  <- [{got.value:2d}] {buf.raw[: got.value].hex(' ')}")
                show_candidates(buf.raw[: got.value])
        print(f"共收到 {seen} 个输入报告")
    finally:
        kernel32.CancelIoEx(h, None)
        kernel32.CloseHandle(h)
    return 0


FILE_FLAG_OVERLAPPED = 0x40000000
ERROR_IO_PENDING = 997
WAIT_OBJECT_0 = 0


class OVERLAPPED(ctypes.Structure):
    _fields_ = [
        ("Internal", ctypes.c_void_p),
        ("InternalHigh", ctypes.c_void_p),
        ("Offset", wintypes.DWORD),
        ("OffsetHigh", wintypes.DWORD),
        ("hEvent", wintypes.HANDLE),
    ]


kernel32.CreateEventW.argtypes = [ctypes.c_void_p, wintypes.BOOL, wintypes.BOOL, wintypes.LPCWSTR]
kernel32.CreateEventW.restype = wintypes.HANDLE
kernel32.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
kernel32.WaitForSingleObject.restype = wintypes.DWORD
kernel32.GetOverlappedResult.argtypes = [
    wintypes.HANDLE,
    ctypes.POINTER(OVERLAPPED),
    ctypes.POINTER(wintypes.DWORD),
    wintypes.BOOL,
]
kernel32.GetOverlappedResult.restype = wintypes.BOOL
kernel32.CancelIoEx.argtypes = [wintypes.HANDLE, ctypes.POINTER(OVERLAPPED)]
kernel32.ReadFile.argtypes = [
    wintypes.HANDLE,
    ctypes.c_void_p,
    wintypes.DWORD,
    ctypes.POINTER(wintypes.DWORD),
    ctypes.POINTER(OVERLAPPED),
]
kernel32.ReadFile.restype = wintypes.BOOL
kernel32.WriteFile.argtypes = [
    wintypes.HANDLE,
    ctypes.c_void_p,
    wintypes.DWORD,
    ctypes.POINTER(wintypes.DWORD),
    ctypes.POINTER(OVERLAPPED),
]
kernel32.WriteFile.restype = wintypes.BOOL


def sniff(vidpid, up, us, seconds=6.0, in_len=None):
    """被动监听输入报告，最多 seconds 秒，把每一帧打印出来。"""
    d = pick(vidpid, up, us, in_len)
    if not d:
        print("找不到该端点")
        return 1
    n = d["in_len"] or 64
    print(f"监听 {d['vid']:04X}:{d['pid']:04X} up={d['usage_page']:04X} u={d['usage']:04X} len={n}")
    h = kernel32.CreateFileW(
        d["path"],
        GENERIC_READ | GENERIC_WRITE,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
        None,
        OPEN_EXISTING,
        FILE_FLAG_OVERLAPPED,
        None,
    )
    if h == INVALID_HANDLE_VALUE:
        print("打开失败")
        return 1
    seen = {}
    try:
        import time

        end = time.time() + seconds
        while time.time() < end:
            ev = kernel32.CreateEventW(None, True, False, None)
            ov = OVERLAPPED()
            ov.hEvent = ev
            buf = ctypes.create_string_buffer(n)
            got = wintypes.DWORD()
            ok = kernel32.ReadFile(h, buf, n, ctypes.byref(got), ctypes.byref(ov))
            if not ok:
                if kernel32.WaitForSingleObject(ev, 1000) != WAIT_OBJECT_0:
                    kernel32.CancelIoEx(h, ctypes.byref(ov))
                    kernel32.GetOverlappedResult(h, ctypes.byref(ov), ctypes.byref(got), True)
                    kernel32.CloseHandle(ev)
                    continue
                kernel32.GetOverlappedResult(h, ctypes.byref(ov), ctypes.byref(got), False)
            kernel32.CloseHandle(ev)
            if got.value:
                raw = buf.raw[: got.value]
                key = raw.hex(" ")
                seen[key] = seen.get(key, 0) + 1
                print(f"  [{got.value:2d}] {key}")
    finally:
        kernel32.CancelIoEx(h, None)
        kernel32.CloseHandle(h)
    print("—— 去重后的帧型：")
    for k, c in seen.items():
        print(f"  x{c:<4} {k}")
    return 0


def show_candidates(raw):
    for i, b in enumerate(raw):
        if 1 <= b <= 100:
            print(f"    byte[{i}] = {b}  (像个电量)")


if __name__ == "__main__":
    a = sys.argv[1:]
    if not a or a[0] == "list":
        cmd_list()
        sys.exit(0)
    if a[0] == "xfer":
        secs = float(a[6]) if len(a) > 6 else 1.0
        il = int(a[5]) if len(a) > 5 else None
        sys.exit(xfer(a[1], int(a[2], 16), int(a[3], 16), a[4], il, secs))
    if a[0] == "sniff":
        secs = float(a[4]) if len(a) > 4 else 6.0
        il = int(a[5]) if len(a) > 5 else None
        sys.exit(sniff(a[1], int(a[2], 16), int(a[3], 16), secs, il))
    mode = a[0]
    vidpid = a[1]
    up = int(a[2], 16)
    us = int(a[3], 16)
    payload = hexbytes(a[4]) if len(a) > 4 else b""
    sys.exit(run(mode, vidpid, up, us, payload))
