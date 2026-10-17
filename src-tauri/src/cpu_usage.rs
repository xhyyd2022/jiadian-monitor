//! 可选的 CPU 自算：仅 hybrid_compat 开启时访问 PDH、系统时间及处理器拓扑。
//! 单控制线程持有查询；模式、代数与全部输出缓存由同一把锁保护。
//! 关闭可中断等待，但不能取消已准入的在途系统调用；旧代结果一律丢弃。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    ERROR_INSUFFICIENT_BUFFER, FILETIME, GetLastError,
};
use windows_sys::Win32::System::Performance::{
    PDH_FMT_COUNTERVALUE, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER,
    PDH_HQUERY, PDH_MORE_DATA, PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData,
    PdhGetFormattedCounterArrayW, PdhGetFormattedCounterValue, PdhOpenQueryW,
};
use windows_sys::Win32::System::SystemInformation::{
    CpuSetInformation, GetSystemCpuSetInformation, SYSTEM_CPU_SET_INFORMATION,
};
use windows_sys::Win32::System::Threading::{
    ALL_PROCESSOR_GROUPS, GetActiveProcessorCount, GetActiveProcessorGroupCount, GetSystemTimes,
};

const SAMPLE_MS: u64 = 1000;
const STALE_MS: u128 = 5000;
const MAX_BUFFER: usize = 16 * 1024 * 1024;
const REBUILD_FAILURES: u32 = 3;
const PATH_UTILITY: &str = r"\Processor Information(_Total)\% Processor Utility";
const PATH_TIME: &str = r"\Processor Information(_Total)\% Processor Time";
const PATH_PERF: &str = r"\Processor Information(_Total)\% Processor Performance";
const PATH_UTILITY_ALL: &str = r"\Processor Information(*)\% Processor Utility";
type Processor = (u16, u8);

/// 百分数。保留 PDH_FMT_DOUBLE 默认封顶语义，不把这些值称为原始 uncapped 数据。
#[derive(Clone, Copy, Debug, Default)]
pub struct Sample {
    pub utility: Option<f64>,
    pub time: Option<f64>,
    pub perf: Option<f64>,
    pub sys_busy: Option<f64>,
    pub load_p: Option<f64>,
    pub load_e: Option<f64>,
}

fn valid(value: Option<f64>) -> Option<f64> {
    value.filter(|v| v.is_finite() && *v >= 0.0)
}

impl Sample {
    pub fn load(&self) -> Option<f64> {
        valid(self.utility).or_else(|| valid(self.time)).or_else(|| valid(self.sys_busy))
    }

    pub fn source(&self) -> &'static str {
        if valid(self.utility).is_some() {
            "自算 · PDH % Processor Utility（效用口径）"
        } else if valid(self.time).is_some() {
            "自算 · PDH % Processor Time（时间口径）"
        } else if valid(self.sys_busy).is_some() {
            "自算 · GetSystemTimes（时间口径兜底）"
        } else {
            "自算 · 采样未就绪"
        }
    }
}

#[derive(Clone, Default)]
struct Diagnostics {
    errors: BTreeMap<&'static str, String>,
    topology: String,
    counts: String,
    system_times: String,
    coverage: String,
}

impl Diagnostics {
    fn record<T>(&mut self, stage: &'static str, result: Result<T, String>) -> Option<T> {
        match result {
            Ok(value) => {
                self.errors.remove(stage);
                Some(value)
            }
            Err(error) => {
                self.errors.insert(stage, error);
                None
            }
        }
    }
}

struct Snapshot {
    epoch: u64,
    at: Instant,
    sample: Sample,
    per_proc: Vec<(String, f64)>,
    diagnostics: Diagnostics,
}

#[derive(Default)]
struct State {
    initialized: bool,
    enabled: bool,
    epoch: u64,
    sequence: u64,
    worker_running: bool,
    worker_error: Option<String>,
    snapshot: Option<Snapshot>,
}

impl State {
    fn set_mode(&mut self, enabled: bool) {
        self.initialized = true;
        if self.enabled != enabled {
            // 不回绕，避免理论上的代数 ABA。
            self.epoch = self.epoch.checked_add(1).expect("CPU 模式代数溢出");
            self.enabled = enabled;
            self.snapshot = None;
        }
    }

    fn accepts(&self, epoch: u64) -> bool {
        self.enabled && self.epoch == epoch
    }

    fn publish(&mut self, snapshot: Snapshot) -> bool {
        if !self.accepts(snapshot.epoch) {
            return false;
        }
        self.sequence = self.sequence.checked_add(1).expect("CPU 采样序号溢出");
        self.snapshot = Some(snapshot);
        true
    }

    fn current(&self) -> Option<&Snapshot> {
        self.snapshot.as_ref().filter(|s| {
            self.accepts(s.epoch) && s.at.elapsed().as_millis() <= STALE_MS
        })
    }
}

struct Controller {
    state: Mutex<State>,
    wake: Condvar,
}

fn controller() -> &'static Controller {
    static C: OnceLock<Controller> = OnceLock::new();
    C.get_or_init(|| Controller { state: Mutex::new(State::default()), wake: Condvar::new() })
}

fn lock_state() -> MutexGuard<'static, State> {
    controller().state.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Clone, Copy)]
struct Epoch(u64);

impl Epoch {
    fn live(self) -> bool {
        lock_state().accepts(self.0)
    }

    // 每个慢 API 前重新准入，不持状态锁调用系统；关闭与准入仍可能相邻发生。
    fn check(self) -> Result<(), String> {
        if self.live() { Ok(()) } else { Err("模式已改变，丢弃在途结果".into()) }
    }
}

fn initialize() {
    // 不在 CPU 锁内读 settings，避免与写盘后的 set_enabled 形成锁顺序依赖。
    let enabled = crate::settings::current().hybrid_compat;
    let mut state = lock_state();
    if !state.initialized {
        state.set_mode(enabled);
    }
}

/// 由 settings::apply_patch 在成功写盘后调用。重复传入 true 也可重试线程创建。
/// 关闭立即清空缓存并唤醒控制器；查询在原 worker 中释放，不跨线程传递句柄。
pub fn set_enabled(enabled: bool) {
    let mut state = lock_state();
    state.set_mode(enabled);
    ensure_worker(&mut state, Duration::from_millis(SAMPLE_MS));
    controller().wake.notify_all();
}

fn ensure_worker(state: &mut State, first_gap: Duration) {
    if !state.enabled || state.worker_running {
        return;
    }
    state.worker_running = true;
    match std::thread::Builder::new().name("cpu-usage".into()).spawn(move || worker(first_gap)) {
        Ok(_) => state.worker_error = None,
        Err(error) => {
            state.worker_running = false;
            state.worker_error = Some(format!("创建 CPU 控制线程失败：{error}（OS={:?}）", error.raw_os_error()));
        }
    }
}

struct WorkerRegistration;

impl Drop for WorkerRegistration {
    fn drop(&mut self) {
        // 若线程意外退出，后续 spawn/set_enabled/sample_once 可再次尝试。
        // Session 的局部变量先析构，查询不会与下一线程重叠。
        let mut state = lock_state();
        state.worker_running = false;
        controller().wake.notify_all();
    }
}

fn wait_epoch(epoch: Epoch, gap: Duration) -> bool {
    let state = lock_state();
    let (state, _) = controller().wake.wait_timeout_while(state, gap, |s| s.accepts(epoch.0))
        .unwrap_or_else(|e| e.into_inner());
    state.accepts(epoch.0)
}

fn worker(first_gap: Duration) {
    let _registration = WorkerRegistration;
    let mut first_gap = Some(first_gap);
    loop {
        let epoch = {
            let state = lock_state();
            let state = controller().wake.wait_while(state, |s| !s.enabled)
                .unwrap_or_else(|e| e.into_inner());
            Epoch(state.epoch)
        };
        let mut session = Session::new(epoch);
        // 第一次只建基线：标量、逐核以及系统差值均不输出首个快照。
        let _ = session.tick();
        let mut gap = first_gap.take().unwrap_or(Duration::from_millis(SAMPLE_MS));
        while wait_epoch(epoch, gap) {
            let snapshot = session.tick();
            let mut state = lock_state();
            if !state.publish(snapshot) {
                break;
            }
            controller().wake.notify_all();
            gap = Duration::from_millis(SAMPLE_MS);
        }
        // 禁用或跨代后先 Drop 查询，再进入休眠/新代，绝不继续 collect。
        drop(session);
    }
}

/// GUI 启动入口；首次读取 settings，关闭时不创建采样线程、不触碰硬件 API。
pub fn spawn() {
    initialize();
    ensure_worker(&mut lock_state(), Duration::from_millis(SAMPLE_MS));
}

/// 诊断入口。关闭时只等待 gap，保留网络诊断所需的等待时间。
/// 开启时复用唯一 worker，至少等 gap，并等待调用后本代的一个完整采样周期。
pub fn sample_once(gap: Duration) {
    initialize();
    let (epoch, sequence) = {
        let mut state = lock_state();
        if !state.enabled {
            drop(state);
            std::thread::sleep(gap);
            return;
        }
        ensure_worker(&mut state, gap);
        (state.epoch, state.sequence)
    };
    std::thread::sleep(gap);
    let state = lock_state();
    let (state, _) = controller().wake.wait_timeout_while(state, Duration::from_secs(8), |s| {
        s.accepts(epoch) && s.worker_running && s.sequence == sequence
    }).unwrap_or_else(|e| e.into_inner());
    drop(state);
}

/// 指标请求用此代数识别读取 LHS 期间发生的切换（包含关→开→关）。
pub fn mode_token() -> (bool, u64) {
    initialize();
    let state = lock_state();
    (state.enabled, state.epoch)
}

pub fn read_for_mode(token: (bool, u64)) -> Result<Option<Sample>, ()> {
    let state = lock_state();
    if (state.enabled, state.epoch) != token {
        return Err(());
    }
    Ok(if token.0 { state.current().map(|s| s.sample) } else { None })
}

/// Convenience: return current Sample, or None if unavailable.
pub fn read() -> Option<Sample> {
    let token = mode_token();
    read_for_mode(token).ok().flatten()
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn api_status(api: &str, status: u32) -> String {
    format!("{api}: 0x{status:08X} ({status})")
}

/// 所有权只在 worker 内，不实现 Send，也不把 PDH 句柄放入全局状态。
struct Pdh {
    query: PDH_HQUERY,
    utility: Option<PDH_HCOUNTER>,
    time: Option<PDH_HCOUNTER>,
    perf: Option<PDH_HCOUNTER>,
    per_proc: Option<PDH_HCOUNTER>,
}

impl Pdh {
    fn open(epoch: Epoch, diagnostics: &mut Diagnostics) -> Result<Self, String> {
        epoch.check()?;
        let mut query = std::ptr::null_mut();
        let status = unsafe { PdhOpenQueryW(std::ptr::null(), 0, &mut query) };
        if status != 0 {
            return Err(api_status("PdhOpenQueryW", status));
        }
        if query.is_null() {
            return Err("PdhOpenQueryW: 成功但查询句柄为空".into());
        }
        let mut pdh = Self { query, utility: None, time: None, perf: None, per_proc: None };
        pdh.utility = diagnostics.record("addUtility", pdh.add(epoch, PATH_UTILITY));
        pdh.time = diagnostics.record("addTime", pdh.add(epoch, PATH_TIME));
        pdh.perf = diagnostics.record("addPerf", pdh.add(epoch, PATH_PERF));
        pdh.per_proc = diagnostics.record("addPerProcessor", pdh.add(epoch, PATH_UTILITY_ALL));
        epoch.check()?;
        if pdh.utility.is_none() && pdh.time.is_none() {
            return Err("PDH 无可用主/备选句柄；具体 PdhAddEnglishCounterW 错误见 addUtility/addTime".into());
        }
        Ok(pdh)
    }

    fn add(&self, epoch: Epoch, path: &str) -> Result<PDH_HCOUNTER, String> {
        epoch.check()?;
        let path = wide(path);
        let mut counter = std::ptr::null_mut();
        let status = unsafe { PdhAddEnglishCounterW(self.query, path.as_ptr(), 0, &mut counter) };
        if status != 0 {
            Err(api_status("PdhAddEnglishCounterW", status))
        } else if counter.is_null() {
            Err("PdhAddEnglishCounterW: 成功但计数器句柄为空".into())
        } else {
            Ok(counter)
        }
    }

    fn repair_missing(&mut self, epoch: Epoch, diagnostics: &mut Diagnostics) {
        // 单独补建缺失的辅助/备用句柄，不销毁仍有效的 total 查询。
        if self.utility.is_none() {
            self.utility = diagnostics.record("addUtility", self.add(epoch, PATH_UTILITY));
        }
        if self.time.is_none() {
            self.time = diagnostics.record("addTime", self.add(epoch, PATH_TIME));
        }
        if self.perf.is_none() {
            self.perf = diagnostics.record("addPerf", self.add(epoch, PATH_PERF));
        }
        if self.per_proc.is_none() {
            self.per_proc = diagnostics.record("addPerProcessor", self.add(epoch, PATH_UTILITY_ALL));
        }
    }

    fn collect(&self, epoch: Epoch) -> Result<(), String> {
        epoch.check()?;
        let status = unsafe { PdhCollectQueryData(self.query) };
        if status == 0 { Ok(()) } else { Err(api_status("PdhCollectQueryData", status)) }
    }

    fn value(epoch: Epoch, counter: Option<PDH_HCOUNTER>) -> Result<f64, String> {
        let counter = counter.ok_or_else(|| "PDH 计数器句柄缺失".to_string())?;
        epoch.check()?;
        let mut value = PDH_FMT_COUNTERVALUE::default();
        let status = unsafe {
            PdhGetFormattedCounterValue(counter, PDH_FMT_DOUBLE, std::ptr::null_mut(), &mut value)
        };
        if status != 0 {
            return Err(api_status("PdhGetFormattedCounterValue", status));
        }
        if value.CStatus > 1 {
            return Err(api_status("PDH CStatus", value.CStatus));
        }
        valid(Some(unsafe { value.Anonymous.doubleValue }))
            .ok_or_else(|| "PDH 返回非有限或负数".into())
    }
}

impl Drop for Pdh {
    fn drop(&mut self) {
        // 即使关闭也必须释放；Drop 不再 Collect，也不访问任何输出缓存。
        unsafe { PdhCloseQuery(self.query) };
    }
}

/// 对齐覆盖当前 Windows API 结构；底层字节全初始化，长度上限限制错误返回的分配。
#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct BufferBlock([u8; 16]);

struct AlignedBuffer {
    blocks: Vec<BufferBlock>,
    len: usize,
}

impl AlignedBuffer {
    fn new<T>(len: usize) -> Result<Self, String> {
        if len == 0 || len > MAX_BUFFER || std::mem::align_of::<T>() > 16 {
            return Err("系统缓冲区长度或对齐无效".into());
        }
        Ok(Self { blocks: vec![BufferBlock([0; 16]); len.div_ceil(16)], len })
    }

    fn ptr<T>(&mut self) -> *mut T {
        self.blocks.as_mut_ptr().cast()
    }

    fn bytes(&self, len: usize) -> Result<&[u8], String> {
        if len > self.len {
            return Err("系统返回长度超过传入缓冲区".into());
        }
        Ok(unsafe { std::slice::from_raw_parts(self.blocks.as_ptr().cast(), len) })
    }
}

fn array_header_bytes(count: usize, len: usize) -> Result<usize, String> {
    let size = count.checked_mul(std::mem::size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>())
        .filter(|n| *n <= len).ok_or_else(|| "PDH 数组条数越界".to_string())?;
    Ok(size)
}

/// 用缓冲区偏移解析名称，绝不解引用 PDH 返回的任意指针。
fn bounded_name(bytes: &[u8], base: usize, address: usize, headers: usize) -> Result<String, String> {
    let start = address.checked_sub(base).filter(|n| *n >= headers && *n < bytes.len());
    let start = start.filter(|_| address % std::mem::align_of::<u16>() == 0)
        .ok_or_else(|| "PDH 实例名指针越界或未对齐".to_string())?;
    let mut units = Vec::new();
    for pair in bytes[start..].chunks_exact(2).take(1024) {
        let unit = u16::from_ne_bytes([pair[0], pair[1]]);
        if unit == 0 {
            return String::from_utf16(&units).map_err(|_| "PDH 实例名 UTF-16 无效".into());
        }
        units.push(unit);
    }
    Err("PDH 实例名缺少有界终止符".into())
}

fn per_processor_utility(epoch: Epoch, counter: PDH_HCOUNTER) -> Result<Vec<(String, f64)>, String> {
    // 热插拔等可能使两次调用间所需长度变化；仅作有限次立即重试。
    for _ in 0..3 {
        epoch.check()?;
        let (mut size, mut count) = (0u32, 0u32);
        let status = unsafe {
            PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut size, &mut count, std::ptr::null_mut())
        };
        if status != PDH_MORE_DATA && status != 0 {
            return Err(api_status("PdhGetFormattedCounterArrayW/size", status));
        }
        let mut buffer = AlignedBuffer::new::<PDH_FMT_COUNTERVALUE_ITEM_W>(size as usize)?;
        epoch.check()?;
        let ptr = buffer.ptr::<PDH_FMT_COUNTERVALUE_ITEM_W>();
        let status = unsafe {
            PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut size, &mut count, ptr)
        };
        if status == PDH_MORE_DATA {
            continue;
        }
        if status != 0 {
            return Err(api_status("PdhGetFormattedCounterArrayW/data", status));
        }
        let bytes = buffer.bytes(size as usize)?;
        let headers = array_header_bytes(count as usize, bytes.len())?;
        if count == 0 {
            return Err("PDH 逐核数组为空".into());
        }
        let mut out = Vec::with_capacity(count as usize);
        for index in 0..count as usize {
            // 已验证完整结构区；名称只允许位于结构区之后的同一缓冲区。
            let item = unsafe { std::ptr::read(ptr.add(index)) };
            let name = bounded_name(bytes, ptr as usize, item.szName as usize, headers)?;
            if item.FmtValue.CStatus > 1 {
                return Err(format!("{name}: {}", api_status("PDH CStatus", item.FmtValue.CStatus)));
            }
            let value = valid(Some(unsafe { item.FmtValue.Anonymous.doubleValue }))
                .ok_or_else(|| format!("PDH 实例 {name} 返回非有限或负数"))?;
            out.push((name, value));
        }
        return Ok(out);
    }
    Err(api_status("PdhGetFormattedCounterArrayW/重试耗尽", PDH_MORE_DATA))
}

fn logical_processor(name: &str) -> Option<Processor> {
    let (group, index) = name.split_once(',')?;
    if group.is_empty() || index.is_empty()
        || !group.bytes().all(|c| c.is_ascii_digit())
        || !index.bytes().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    Some((group.parse().ok()?, index.parse().ok()?))
}

struct Topology {
    classes: HashMap<Processor, u8>,
    min: u8,
    max: u8,
}

fn parse_topology(bytes: &[u8], expected: u32) -> Result<Topology, String> {
    let mut classes = HashMap::new();
    let mut offset = 0usize;
    while offset < bytes.len() {
        let header = bytes.get(offset..).filter(|b| b.len() >= 8)
            .ok_or_else(|| "CPU 拓扑记录头截断".to_string())?;
        let size = u32::from_ne_bytes(header[..4].try_into().unwrap()) as usize;
        let kind = i32::from_ne_bytes(header[4..8].try_into().unwrap());
        let end = offset.checked_add(size).filter(|n| size >= 8 && *n <= bytes.len())
            .ok_or_else(|| "CPU 拓扑记录长度无效".to_string())?;
        if kind == CpuSetInformation {
            if size < std::mem::size_of::<SYSTEM_CPU_SET_INFORMATION>() {
                return Err("CPU Set 记录截断".into());
            }
            let info = unsafe {
                std::ptr::read_unaligned(bytes.as_ptr().add(offset).cast::<SYSTEM_CPU_SET_INFORMATION>())
            };
            let cpu = unsafe { info.Anonymous.CpuSet };
            if classes.insert((cpu.Group, cpu.LogicalProcessorIndex), cpu.EfficiencyClass).is_some() {
                return Err("CPU 拓扑含重复逻辑处理器".into());
            }
        }
        offset = end;
    }
    if classes.is_empty() || classes.len() != expected as usize {
        return Err(format!("CPU 拓扑覆盖不完整：{} / {expected}", classes.len()));
    }
    let min = *classes.values().min().unwrap();
    let max = *classes.values().max().unwrap();
    Ok(Topology { classes, min, max })
}

fn fast_core_map(epoch: Epoch, expected: u32) -> Result<Topology, String> {
    for _ in 0..3 {
        epoch.check()?;
        let mut len = 0u32;
        let ok = unsafe {
            GetSystemCpuSetInformation(std::ptr::null_mut(), 0, &mut len, std::ptr::null_mut(), 0)
        };
        if ok == 0 {
            let error = unsafe { GetLastError() };
            if error != ERROR_INSUFFICIENT_BUFFER {
                return Err(api_status("GetSystemCpuSetInformation/size Win32", error));
            }
        }
        let mut buffer = AlignedBuffer::new::<SYSTEM_CPU_SET_INFORMATION>(len as usize)?;
        epoch.check()?;
        let ok = unsafe {
            GetSystemCpuSetInformation(buffer.ptr(), len, &mut len, std::ptr::null_mut(), 0)
        };
        if ok == 0 {
            let error = unsafe { GetLastError() };
            if error == ERROR_INSUFFICIENT_BUFFER {
                continue;
            }
            return Err(api_status("GetSystemCpuSetInformation/data Win32", error));
        }
        return parse_topology(buffer.bytes(len as usize)?, expected);
    }
    Err(api_status("GetSystemCpuSetInformation/重试耗尽 Win32", ERROR_INSUFFICIENT_BUFFER))
}

fn average(values: impl Iterator<Item = f64>) -> Option<f64> {
    // 在线等权平均，避免有限大数相加溢出；不改变原有组均值口径或额外封顶。
    let (mut mean, mut count) = (0.0, 0usize);
    for value in values {
        let value = valid(Some(value))?;
        count += 1;
        mean += (value - mean) / count as f64;
    }
    if count == 0 { None } else { valid(Some(mean)) }
}

fn grouped(topology: &Topology, values: &[(String, f64)]) -> Result<(Option<f64>, Option<f64>), String> {
    let mut seen = HashSet::new();
    let (mut p, mut e) = (Vec::new(), Vec::new());
    for (name, value) in values {
        let Some(id) = logical_processor(name) else { continue };
        let class = topology.classes.get(&id).ok_or_else(|| format!("拓扑缺少 PDH 实例 {name}"))?;
        if !seen.insert(id) {
            return Err(format!("PDH 逻辑处理器实例重复：{name}"));
        }
        let value = valid(Some(*value)).ok_or_else(|| format!("PDH 实例 {name} 数值无效"))?;
        if *class == topology.max { p.push(value); } else { e.push(value); }
    }
    if seen.len() != topology.classes.len() {
        return Err(format!("逐核样本覆盖不完整：{} / {}", seen.len(), topology.classes.len()));
    }
    if topology.min == topology.max {
        Ok((None, None))
    } else {
        Ok((average(p.into_iter()), average(e.into_iter())))
    }
}

#[derive(Clone, Copy)]
struct SysTimes {
    idle: u64,
    total: u64,
}

fn times_from_parts(idle: u64, kernel: u64, user: u64) -> Option<SysTimes> {
    if idle > kernel { return None; }
    Some(SysTimes { idle, total: kernel.checked_add(user)? })
}

fn sys_times(epoch: Epoch) -> Result<SysTimes, String> {
    epoch.check()?;
    let mut idle = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut kernel, mut user) = (idle, idle);
    if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
        return Err(api_status("GetSystemTimes Win32", unsafe { GetLastError() }));
    }
    let ticks = |t: FILETIME| ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64;
    times_from_parts(ticks(idle), ticks(kernel), ticks(user))
        .ok_or_else(|| "GetSystemTimes: 时间合计溢出或 idle > kernel".into())
}

fn busy(a: Option<SysTimes>, b: Option<SysTimes>) -> Option<f64> {
    let (a, b) = (a?, b?);
    let total = b.total.checked_sub(a.total)?;
    let idle = b.idle.checked_sub(a.idle)?;
    let used = total.checked_sub(idle)?;
    if total == 0 { return None; }
    valid(Some(used as f64 / total as f64 * 100.0))
}

fn system_times_supported(groups: u16, processors: u32) -> bool {
    groups == 1 && processors > 0 && processors <= 64
}

fn processor_counts(epoch: Epoch) -> Result<(u16, u32), String> {
    epoch.check()?;
    let groups = unsafe { GetActiveProcessorGroupCount() };
    if groups == 0 {
        return Err(api_status("GetActiveProcessorGroupCount Win32", unsafe { GetLastError() }));
    }
    epoch.check()?;
    let processors = unsafe { GetActiveProcessorCount(ALL_PROCESSOR_GROUPS) };
    if processors == 0 {
        return Err(api_status("GetActiveProcessorCount Win32", unsafe { GetLastError() }));
    }
    Ok((groups, processors))
}

struct Retry {
    failures: u32,
    next: Instant,
}

impl Retry {
    fn new() -> Self { Self { failures: 0, next: Instant::now() } }
    fn ready(&self) -> bool { Instant::now() >= self.next }
    fn failed(&mut self) {
        let seconds = (1u64 << self.failures.min(5)).min(30);
        self.failures = self.failures.saturating_add(1);
        self.next = Instant::now() + Duration::from_secs(seconds);
    }
    fn reset(&mut self) { self.failures = 0; }
}

/// 第一次成功 Collect 仅建基线；失败会使下一次成功再次成为基线。
#[derive(Default)]
struct Baseline(bool);

impl Baseline {
    fn collected(&mut self, ok: bool) -> bool {
        let usable = ok && self.0;
        self.0 = ok;
        usable
    }
}

fn query_unhealthy(collected: bool, usable: bool, sample: &Sample) -> bool {
    !collected || (usable && valid(sample.utility).is_none() && valid(sample.time).is_none())
}

struct Session {
    epoch: Epoch,
    pdh: Option<Pdh>,
    baseline: Baseline,
    pdh_retry: Retry,
    auxiliary_retry: Retry,
    pdh_failures: u32,
    topology: Option<Topology>,
    topology_retry: Retry,
    prev: Option<SysTimes>,
    diagnostics: Diagnostics,
}

impl Session {
    fn new(epoch: Epoch) -> Self {
        Self {
            epoch, pdh: None, baseline: Baseline::default(), pdh_retry: Retry::new(),
            auxiliary_retry: Retry::new(), pdh_failures: 0, topology: None, topology_retry: Retry::new(), prev: None,
            diagnostics: Diagnostics {
                topology: "unknown（尚未成功读取）".into(), counts: "未知".into(),
                system_times: "尚未检查处理器组".into(), coverage: "尚无完整逐核样本".into(),
                ..Diagnostics::default()
            },
        }
    }

    fn tick(&mut self) -> Snapshot {
        let mut sample = Sample::default();
        let mut per_proc = Vec::new();
        if self.epoch.live() {
            self.sample_system(&mut sample);
        }
        if self.epoch.live() {
            self.sample_pdh(&mut sample, &mut per_proc);
        }
        Snapshot {
            epoch: self.epoch.0, at: Instant::now(), sample, per_proc,
            diagnostics: self.diagnostics.clone(),
        }
    }

    fn sample_system(&mut self, sample: &mut Sample) {
        // 不依赖 CPU Set 拓扑成功与否，独立确认是否允许 GetSystemTimes。
        let counts = self.diagnostics.record("processorCounts", processor_counts(self.epoch));
        let Some((groups, processors)) = counts else {
            self.prev = None;
            self.topology = None;
            self.diagnostics.topology = "unknown（处理器组信息读取失败）".into();
            self.diagnostics.counts = "未知".into();
            self.diagnostics.system_times = "禁用：处理器组信息未知，不能保证整机口径".into();
            return;
        };
        if self.topology.as_ref().is_some_and(|t| t.classes.len() != processors as usize) {
            self.topology = None;
            self.topology_retry = Retry::new();
        }
        if self.topology.is_none() && self.topology_retry.ready() {
            self.topology = self.diagnostics.record("topology", fast_core_map(self.epoch, processors));
            if self.topology.is_none() { self.topology_retry.failed(); }
        }
        if let Some(topology) = &self.topology {
            let p = topology.classes.values().filter(|c| **c == topology.max).count();
            if topology.min == topology.max {
                self.diagnostics.topology = "同构核（EfficiencyClass 只有一类）".into();
                self.diagnostics.counts = format!("{processors} 逻辑处理器，不分组");
            } else {
                self.diagnostics.topology = "混合核（EfficiencyClass 最大者归 P，其余归 E）".into();
                self.diagnostics.counts = format!("P={p}, E={} 逻辑处理器", topology.classes.len() - p);
            }
        } else {
            self.diagnostics.topology = "unknown（读取失败，将有限退避重试；不是同构核结论）".into();
            self.diagnostics.counts = format!("{processors} 逻辑处理器，{groups} 处理器组");
        }
        if !system_times_supported(groups, processors) {
            self.prev = None;
            self.diagnostics.system_times = format!(
                "禁用：{processors} 逻辑处理器 / {groups} 组；多组或 >64 时 GetSystemTimes 不能作为整机兜底"
            );
            return;
        }
        self.diagnostics.system_times = "可用：单组且不超过 64 逻辑处理器".into();
        let now = self.diagnostics.record("systemTimes", sys_times(self.epoch));
        sample.sys_busy = busy(self.prev, now);
        if self.prev.is_some() && now.is_some() && sample.sys_busy.is_none() {
            self.diagnostics.errors.insert("systemDelta", "系统时间差值回退、溢出、idle 差值超 total 或窗口为零；重建基线".into());
        } else {
            self.diagnostics.errors.remove("systemDelta");
        }
        self.prev = now;
    }

    fn sample_pdh(&mut self, sample: &mut Sample, per_proc: &mut Vec<(String, f64)>) {
        self.diagnostics.coverage = "本轮无完整逐核样本".into();
        if self.pdh.is_none() && self.pdh_retry.ready() {
            let opened = Pdh::open(self.epoch, &mut self.diagnostics);
            self.pdh = self.diagnostics.record("openQuery", opened);
            if self.pdh.is_none() {
                self.pdh_retry.failed();
            }
            self.baseline = Baseline::default();
        }
        if let Some(pdh) = &mut self.pdh {
            if self.auxiliary_retry.ready()
                && (pdh.utility.is_none() || pdh.time.is_none() || pdh.perf.is_none() || pdh.per_proc.is_none()) {
                pdh.repair_missing(self.epoch, &mut self.diagnostics);
                self.auxiliary_retry.failed();
            }
        }
        let Some(pdh) = &self.pdh else { return };
        let collected = self.diagnostics.record("collect", pdh.collect(self.epoch)).is_some();
        let usable = self.baseline.collected(collected);
        if usable {
            sample.utility = self.diagnostics.record("utility", Pdh::value(self.epoch, pdh.utility));
            sample.time = self.diagnostics.record("time", Pdh::value(self.epoch, pdh.time));
            sample.perf = self.diagnostics.record("perf", Pdh::value(self.epoch, pdh.perf));
            if let Some(counter) = pdh.per_proc {
                *per_proc = self.diagnostics.record("perProcessor", per_processor_utility(self.epoch, counter))
                    .unwrap_or_default();
            }
            if let Some(topology) = &self.topology {
                match grouped(topology, per_proc) {
                    Ok((p, e)) => {
                        sample.load_p = p;
                        sample.load_e = e;
                        self.topology_retry.reset();
                        self.diagnostics.coverage = format!("完整：{} 逻辑处理器", topology.classes.len());
                        self.diagnostics.errors.remove("groupCoverage");
                    }
                    Err(error) => {
                        self.diagnostics.coverage = error.clone();
                        self.diagnostics.errors.insert("groupCoverage", error);
                        // 映射可能因热插拔失效。未知与同构分开，重新读取也遵循退避。
                        self.topology = None;
                        self.topology_retry.failed();
                        self.diagnostics.topology = "unknown（样本与拓扑不一致，将重试）".into();
                    }
                }
            }
        }
        // 只有整个查询/两条总负载都失效才重建。perf、逐核与拓扑仅降级诊断/分组。
        let unhealthy = query_unhealthy(collected, usable, sample);
        if unhealthy {
            self.pdh_failures = self.pdh_failures.saturating_add(1);
        } else if usable {
            self.pdh_failures = 0;
            self.pdh_retry.reset();
        }
        if self.pdh_failures >= REBUILD_FAILURES {
            // 在 worker 上立刻 Drop；下次打开必须重新经过首样本基线。
            self.pdh = None;
            self.baseline = Baseline::default();
            self.pdh_failures = 0;
            self.pdh_retry.failed();
            self.diagnostics.errors.insert("rebuild", "PDH 持续失败或缺少主句柄，已释放查询并安排重建（1～30 秒退避）".into());
        } else if usable && !unhealthy {
            self.diagnostics.errors.remove("rebuild");
        }
    }
}

/// 仅序列化同锁内的本代缓存；不会为诊断偷偷访问拓扑或采样 API。
pub fn dump_json() -> String {
    let state = lock_state();
    let snapshot = state.current();
    let sample = snapshot.map(|s| s.sample).unwrap_or_default();
    let per = snapshot.map(|s| s.per_proc.as_slice()).unwrap_or_default();
    let diagnostics = snapshot.map(|s| &s.diagnostics);
    let mean = average(per.iter().filter(|(name, _)| logical_processor(name).is_some()).map(|(_, v)| *v));
    let per_sample: Vec<_> = per.iter().take(8).map(|(name, v)| format!("{name}={v:.1}")).collect();
    let per_other: Vec<_> = per.iter().filter(|(name, _)| logical_processor(name).is_none())
        .map(|(name, v)| format!("{name}={v:.1}")).collect();
    serde_json::json!({
        "utility": sample.utility,
        "time": sample.time,
        "perf": sample.perf,
        "sysBusy": sample.sys_busy,
        "used": sample.load(),
        "source": sample.source(),
        "hybridCompat": state.enabled,
        "epoch": state.epoch,
        "workerRunning": state.worker_running,
        "workerError": state.worker_error,
        "error": if !state.enabled { Some("大小核兼容模式已关闭，未进行 CPU 自算") }
            else if snapshot.is_none() { Some("尚无本代有效缓存，或缓存已超过 5 秒") } else { None },
        "coreTopology": diagnostics.map(|d| d.topology.as_str()).unwrap_or("unknown（未采样）"),
        "coreCounts": diagnostics.map(|d| d.counts.as_str()).unwrap_or("未知"),
        "loadP": sample.load_p,
        "loadE": sample.load_e,
        "perProcessorCount": per.len(),
        "perProcessorMean": mean,
        "perProcessorSample": per_sample,
        "perProcessorOther": per_other,
        "groupCoverage": diagnostics.map(|d| d.coverage.as_str()),
        "getSystemTimes": diagnostics.map(|d| d.system_times.as_str()),
        "errors": diagnostics.map(|d| &d.errors),
        "valueFormat": "PDH_FMT_DOUBLE，保留默认封顶；逐核值与总值均非原始 uncapped；组均值不另加封顶",
        "comparison": "效用与时间口径不同，不保证与任务管理器恒等，也不保证逐核均值等于 _Total",
        "inflightBoundary": "关闭不能取消已准入的在途系统调用；返回后按 epoch 丢弃，并由唯一 worker 释放查询"
    }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(epoch: u64, value: f64) -> Snapshot {
        Snapshot {
            epoch, at: Instant::now(), sample: Sample { utility: Some(value), ..Sample::default() },
            per_proc: vec![("0,0".into(), value)], diagnostics: Diagnostics::default(),
        }
    }

    #[test]
    fn auxiliary_failures_do_not_discard_valid_total() {
        let utility = Sample { utility: Some(12.0), ..Sample::default() };
        let time = Sample { time: Some(13.0), ..Sample::default() };
        for _ in 0..10 {
            assert!(!query_unhealthy(true, true, &utility));
            assert!(!query_unhealthy(true, true, &time));
        }
        assert!(query_unhealthy(false, false, &utility));
        assert!(query_unhealthy(true, true, &Sample::default()));
        assert!(!query_unhealthy(true, false, &Sample::default()));
    }

    #[test]
    fn mode_and_cache_share_epoch_checks() {
        // 仅使用独立 State，不访问控制器、不 spawn、不读取 settings 或硬件。
        let mut state = State::default();
        assert!(!state.publish(snapshot(0, 10.0)));
        state.set_mode(true);
        let old = state.epoch;
        assert!(state.publish(snapshot(old, 20.0)));
        assert_eq!(state.current().unwrap().per_proc[0].1, 20.0);
        state.set_mode(true);
        assert_eq!(state.epoch, old);
        state.set_mode(false);
        assert!(state.current().is_none());
        assert!(state.snapshot.is_none());
        assert!(!state.publish(snapshot(old, 30.0)));
        state.set_mode(true);
        let new = state.epoch;
        assert!(new > old);
        assert!(!state.publish(snapshot(old, 40.0)));
        assert!(state.publish(snapshot(new, 50.0)));
        assert_eq!(state.current().unwrap().sample.load(), Some(50.0));
        state.snapshot = Some(snapshot(old, 60.0));
        assert!(state.current().is_none());
    }

    #[test]
    fn stale_output_is_not_available() {
        let mut state = State::default();
        state.set_mode(true);
        let mut value = snapshot(state.epoch, 1.0);
        value.at = Instant::now() - Duration::from_secs(6);
        assert!(state.publish(value));
        assert!(state.current().is_none());
    }

    #[test]
    fn invalid_numbers_fall_back_with_matching_source() {
        for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0] {
            let mut sample = Sample { utility: Some(invalid), time: Some(25.0), ..Sample::default() };
            assert_eq!(sample.load(), Some(25.0));
            assert!(sample.source().contains("Processor Time"));
            sample.time = Some(invalid);
            sample.sys_busy = Some(0.0);
            assert_eq!(sample.load(), Some(0.0));
            assert!(sample.source().contains("GetSystemTimes"));
            sample.sys_busy = Some(invalid);
            assert_eq!(sample.load(), None);
            assert!(sample.source().contains("未就绪"));
        }
        // 有限非负不在纯逻辑层额外封顶；PDH 默认格式自行决定封顶。
        assert_eq!(valid(Some(120.0)), Some(120.0));
        assert_eq!(average([f64::MAX, f64::MAX].into_iter()), Some(f64::MAX));
        assert_eq!(average([10.0, f64::NAN].into_iter()), None);
        assert_eq!(average(std::iter::empty()), None);
    }

    #[test]
    fn system_time_deltas_are_checked() {
        let a = Some(SysTimes { idle: 10, total: 100 });
        assert_eq!(busy(None, a), None);
        assert_eq!(busy(a, a), None);
        assert_eq!(busy(a, Some(SysTimes { idle: 9, total: 110 })), None);
        assert_eq!(busy(a, Some(SysTimes { idle: 11, total: 99 })), None);
        assert_eq!(busy(a, Some(SysTimes { idle: 30, total: 110 })), None);
        assert_eq!(busy(a, Some(SysTimes { idle: 15, total: 120 })), Some(75.0));
        assert!(times_from_parts(1, u64::MAX, 1).is_none());
        assert!(times_from_parts(11, 10, 0).is_none());
        assert_eq!(times_from_parts(10, 20, 30).unwrap().total, 50);
        assert!(system_times_supported(1, 64));
        for (groups, count) in [(2, 128), (2, 64), (1, 65), (0, 64), (1, 0)] {
            assert!(!system_times_supported(groups, count));
        }
    }

    #[test]
    fn first_success_and_post_failure_success_are_baselines() {
        let mut baseline = Baseline::default();
        assert!(!baseline.collected(false));
        assert!(!baseline.collected(true));
        assert!(baseline.collected(true));
        assert!(!baseline.collected(false));
        assert!(!baseline.collected(true));
        assert!(baseline.collected(true));
        let mut new_epoch = Baseline::default();
        assert!(!new_epoch.collected(true));
    }

    fn mixed_topology() -> Topology {
        Topology {
            classes: HashMap::from([((0, 0), 2), ((0, 1), 2), ((1, 0), 0)]), min: 0, max: 2,
        }
    }

    fn full_values() -> Vec<(String, f64)> {
        vec![
            ("0,0".into(), 20.0), ("0,1".into(), 60.0), ("1,0".into(), 10.0),
            ("_Total".into(), 99.0), ("0,_Total".into(), 98.0),
        ]
    }

    #[test]
    fn groups_require_exact_full_multi_group_coverage() {
        let topology = mixed_topology();
        assert_eq!(grouped(&topology, &full_values()).unwrap(), (Some(40.0), Some(10.0)));
        let mut values = full_values();
        values.remove(1);
        assert!(grouped(&topology, &values).is_err());
        let mut values = full_values();
        values.push(("0,0".into(), 20.0));
        assert!(grouped(&topology, &values).is_err());
        let mut values = full_values();
        values.push(("1,1".into(), 20.0));
        assert!(grouped(&topology, &values).is_err());
        for invalid in [f64::NAN, f64::INFINITY, -0.1] {
            let mut values = full_values();
            values[0].1 = invalid;
            assert!(grouped(&topology, &values).is_err());
        }
        let uniform = Topology { classes: HashMap::from([((0, 0), 0)]), min: 0, max: 0 };
        assert_eq!(grouped(&uniform, &[("0,0".into(), 10.0)]).unwrap(), (None, None));
        assert!(grouped(&uniform, &[]).is_err());
    }

    #[test]
    fn instance_names_are_strictly_parsed() {
        assert_eq!(logical_processor("1,63"), Some((1, 63)));
        for name in ["_Total", "0,_Total", "0", "0,1,2", "0,-1", "0,256", "65536,0", " 0,1", ",1", "0,"] {
            assert_eq!(logical_processor(name), None);
        }
    }

    #[test]
    fn array_and_name_bounds_are_checked() {
        let item = std::mem::size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>();
        assert!(array_header_bytes(usize::MAX, item).is_err());
        assert!(array_header_bytes(2, item).is_err());
        assert_eq!(array_header_bytes(1, item).unwrap(), item);
        let bytes = [0u8, 0, b'0', 0, b',', 0, b'1', 0, 0, 0];
        assert_eq!(bounded_name(&bytes, 100, 102, 2).unwrap(), "0,1");
        for address in [0, 99, 100, 101, 110, usize::MAX] {
            assert!(bounded_name(&bytes, 100, address, 2).is_err());
        }
        assert!(bounded_name(&bytes[..8], 100, 102, 2).is_err());
        assert!(bounded_name(&[0, 0, 0, 0xD8, 0, 0], 100, 102, 2).is_err());
        assert!(AlignedBuffer::new::<SYSTEM_CPU_SET_INFORMATION>(MAX_BUFFER + 1).is_err());
        let mut buffer = AlignedBuffer::new::<SYSTEM_CPU_SET_INFORMATION>(64).unwrap();
        assert_eq!(buffer.ptr::<SYSTEM_CPU_SET_INFORMATION>() as usize % std::mem::align_of::<SYSTEM_CPU_SET_INFORMATION>(), 0);
        assert!(buffer.bytes(65).is_err());
    }

    #[test]
    fn malformed_topology_is_unknown_not_uniform() {
        assert!(parse_topology(&[], 1).is_err());
        assert!(parse_topology(&[0; 7], 1).is_err());
        assert!(parse_topology(&[0; 8], 1).is_err());
        let mut record = vec![0u8; std::mem::size_of::<SYSTEM_CPU_SET_INFORMATION>()];
        let size = record.len() as u32;
        record[..4].copy_from_slice(&size.to_ne_bytes());
        record[4..8].copy_from_slice(&CpuSetInformation.to_ne_bytes());
        let uniform = parse_topology(&record, 1).unwrap();
        assert_eq!(uniform.min, uniform.max);
        assert!(parse_topology(&record, 2).is_err());
        let duplicate = [record.as_slice(), record.as_slice()].concat();
        assert!(parse_topology(&duplicate, 2).is_err());
    }
}
