//! 同步文件日志：`log/jiadian.log`，正常记录在写入前按 1 MiB 轮转，保留一个 `.1`。
//!
//! 文件状态、写入与轮转共用进程内锁；故障时暂停文件输出并冷却重试，stderr 尽力输出。
//! 不使用无界队列、不 sleep、不把文件 I/O 失败升级为 panic。
//! 注意：这是进程内串行化，尚未提供 GUI/CLI 之间的跨进程轮转协议。

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::SYSTEMTIME;
use windows_sys::Win32::System::SystemInformation::GetLocalTime;

const MAX_BYTES: u64 = 1024 * 1024;
const MAX_RECORD_BYTES: usize = 16 * 1024;
// 正常文件预留一条有界 panic 记录的空间，避免刚好满额时丢掉首次崩溃线索。
const NORMAL_BYTES: u64 = MAX_BYTES - MAX_RECORD_BYTES as u64;
const RETRY_DELAY: Duration = Duration::from_secs(5);
const TRUNCATED: &str = "…[已截断]";

type Diagnostic = (&'static str, String);

static SINK: Mutex<FileSink> = Mutex::new(FileSink::new());
static ENABLED: AtomicBool = AtomicBool::new(true);
static LEVEL: AtomicU8 = AtomicU8::new(0);

/// 不缓存长度：CLI/异常路径可能在普通 logger 之外追加，失败的 write_all 也可能已写入前缀。
/// 每次准入使用句柄的真实长度；降低 metadata 频率须与跨进程协议一起设计，不能盲目记账。
struct FileSink {
    path: Option<PathBuf>,
    file: Option<File>,
    retry_at: Option<Instant>,
    last_error: Option<String>,
    dropped: u64,
    max_bytes: u64,
}

impl FileSink {
    const fn new() -> Self {
        Self {
            path: None,
            file: None,
            retry_at: None,
            last_error: None,
            dropped: 0,
            max_bytes: NORMAL_BYTES,
        }
    }

    fn close(&mut self) {
        self.file = None;
        self.retry_at = None;
    }

    fn ensure_open(&mut self) -> Result<(), String> {
        if self.file.is_some() {
            return Ok(());
        }
        let path = self.path.get_or_insert_with(|| {
            dir()
                .unwrap_or_else(|| PathBuf::from("log"))
                .join("jiadian.log")
        });
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)
                .map_err(|e| format!("创建日志目录 {} 失败：{e}", parent.display()))?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&*path)
            .map_err(|e| format!("打开日志 {} 失败：{e}", path.display()))?;
        // 获取长度失败不能当成空文件；不发布缺少容量信息的句柄。
        file.metadata()
            .map_err(|e| format!("读取日志长度 {} 失败：{e}", path.display()))?;
        self.file = Some(file);
        Ok(())
    }

    fn append(&mut self, text: &str) -> Result<(), String> {
        let len = text.len() as u64;
        if len > self.max_bytes {
            return Err(format!("日志记录过大（{len} > {}）", self.max_bytes));
        }
        self.ensure_open()?;
        let file = self.file.as_mut().ok_or("日志句柄未就绪")?;
        let size = file
            .metadata()
            .map_err(|e| format!("读取日志长度失败：{e}"))?
            .len();
        let rotated = size.saturating_add(len) > self.max_bytes;
        if rotated {
            // 检查、关闭、rename、重开与后面的写入始终处于同一个 SINK 锁中。
            self.file = None;
            let path = self.path.as_ref().ok_or("日志路径未就绪")?;
            let mut backup = path.as_os_str().to_os_string();
            backup.push(".1");
            let backup = PathBuf::from(backup);
            fs::rename(path, &backup).map_err(|e| {
                format!(
                    "轮转日志 {} -> {} 失败：{e}",
                    path.display(),
                    backup.display()
                )
            })?;
            // rename 失败绝不能再 append 大文件；成功后重开失败也不删除归档。
            self.ensure_open()?;
        }
        let file = self.file.as_mut().ok_or("日志句柄未就绪")?;
        // 重开后再次验证，不能把外部并发写入/异常追加当成不存在。
        if rotated
            && file
                .metadata()
                .map_err(|e| format!("校验日志长度失败：{e}"))?
                .len()
                .saturating_add(len)
                > self.max_bytes
        {
            return Err("日志在写入前已再次超过容量，暂停文件输出".into());
        }
        file.write_all(text.as_bytes())
            .map_err(|e| format!("追加日志失败（可能已部分写入）：{e}"))?;
        // std::fs::File 不带用户态缓冲；不逐行调用无实际持久化保证的 flush，更不做 fsync。
        Ok(())
    }

    /// 返回需单独输出到 stderr 的诊断；绝不能递归调用本模块的 line/info/warn。
    /// record=None 只准备文件，供 init 使用；这里不读取 settings。
    fn attempt(&mut self, record: Option<&str>, now: Instant) -> Option<Diagnostic> {
        if self.retry_at.is_some_and(|deadline| now < deadline) {
            if record.is_some() {
                self.dropped = self.dropped.saturating_add(1);
            }
            return None;
        }
        let result = match record {
            Some(text) => self.append(text),
            None => self.ensure_open(),
        };
        match result {
            Ok(()) => {
                self.retry_at = None;
                self.last_error.take().map(|previous| {
                    let dropped = std::mem::take(&mut self.dropped);
                    ("INFO", format!("日志文件输出恢复，故障期间未落盘 {dropped} 条（stderr 仍尽力输出）；上次错误：{previous}"))
                })
            }
            Err(error) => {
                // write_all Err 可能已写入部分内容：关闭后冷却，不在这里整行重发制造重复。
                self.file = None;
                self.retry_at = Some(now + RETRY_DELAY);
                if record.is_some() {
                    self.dropped = self.dropped.saturating_add(1);
                }
                self.last_error = Some(error.clone());
                Some((
                    "ERROR",
                    format!(
                        "日志文件输出暂停，{} 秒后遇到新记录再重试：{error}",
                        RETRY_DELAY.as_secs()
                    ),
                ))
            }
        }
    }
}

fn lock_sink(mutex: &Mutex<FileSink>) -> MutexGuard<'_, FileSink> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            let mut guard = poisoned.into_inner();
            // 不信任 panic 中断的半提交句柄，重新打开并读取真实长度后才允许写。
            guard.close();
            guard.last_error = Some("日志状态锁曾被 panic 中断，已丢弃旧句柄".into());
            mutex.clear_poison();
            guard
        }
    }
}

fn rank(level: &str) -> u8 {
    match level {
        "WARN" => 1,
        "ERROR" => 2,
        _ => 0,
    }
}

fn level_rank(name: &str) -> u8 {
    match name.trim().to_ascii_lowercase().as_str() {
        "warn" | "warning" => 1,
        "error" => 2,
        _ => 0,
    }
}

fn accepts(on: bool, threshold: u8, level: &str) -> bool {
    on && rank(level) >= threshold
}

pub fn set_level(name: &str) {
    LEVEL.store(level_rank(name), Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// 禁用后等待已有文件操作退出并关闭句柄；已经开始的 stderr 输出可能仍会完成。
pub fn set_enabled(on: bool) {
    let mut sink = lock_sink(&SINK);
    let previous = ENABLED.swap(on, Ordering::Relaxed);
    if !on || !previous {
        sink.close();
    }
}

/// `log/` 目录（cwd，或从 exe 目录逐级向上）。
pub fn dir() -> Option<PathBuf> {
    let mut cands: Vec<PathBuf> = vec![PathBuf::from("log")];
    if let Ok(exe) = std::env::current_exe() {
        let mut d = exe.parent().map(|x| x.to_path_buf());
        for _ in 0..4 {
            let Some(cur) = d else { break };
            cands.push(cur.join("log"));
            d = cur.parent().map(|x| x.to_path_buf());
        }
    }
    cands.into_iter().find(|p| p.is_dir())
}

/// 启动配置读取在文件状态锁之外；内部懒重开绝不调用 init 覆盖运行时设置。
pub fn init() {
    let settings = crate::settings::current();
    set_enabled(settings.logging);
    set_level(&settings.log_level);
    let diagnostic = {
        let mut sink = lock_sink(&SINK);
        if !enabled() {
            return;
        }
        sink.attempt(None, Instant::now())
    };
    emit_diagnostic(diagnostic);
}

fn timestamp() -> String {
    let mut st = SYSTEMTIME {
        wYear: 0,
        wMonth: 0,
        wDayOfWeek: 0,
        wDay: 0,
        wHour: 0,
        wMinute: 0,
        wSecond: 0,
        wMilliseconds: 0,
    };
    unsafe { GetLocalTime(&mut st) };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
        st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond, st.wMilliseconds
    )
}

/// 单条记录限制 16 KiB，按 UTF-8 字符截断；转义换行以免设备文本伪造后续日志。
fn record(level: &str, msg: &str) -> String {
    let tag: String = level.chars().take(16).filter(|c| !c.is_control()).collect();
    let mut text = format!(
        "{} [{tag}] [pid={} tid={:?}] ",
        timestamp(),
        std::process::id(),
        std::thread::current().id()
    );
    let limit = MAX_RECORD_BYTES - TRUNCATED.len() - 1;
    let mut truncated = false;
    for ch in msg.chars() {
        let mut buf = [0; 4];
        let value = match ch {
            '\r' => "\\r",
            '\n' => "\\n",
            '\0' => "\\0",
            _ => ch.encode_utf8(&mut buf),
        };
        if text.len() + value.len() > limit {
            truncated = true;
            break;
        }
        text.push_str(value);
    }
    if truncated {
        text.push_str(TRUNCATED);
    }
    text.push('\n');
    text
}

fn write_best_effort(writer: &mut impl Write, text: &str) {
    let _ = writer.write_all(text.as_bytes());
}

fn stderr(text: &str) {
    write_best_effort(&mut io::stderr().lock(), text);
}

fn emit_diagnostic(diagnostic: Option<Diagnostic>) {
    if let Some((level, message)) = diagnostic {
        if accepts(enabled(), LEVEL.load(Ordering::Relaxed), level) {
            stderr(&record(level, &message));
        }
    }
}

/// 被过滤的记录不格式化、不打开文件、不轮转。调用方自行 format! 的成本不在此保证内。
pub fn line(level: &str, msg: &str) {
    if !accepts(enabled(), LEVEL.load(Ordering::Relaxed), level) {
        return;
    }
    let text = record(level, msg);
    let diagnostic = {
        let mut sink = lock_sink(&SINK);
        // 开关与文件句柄发布处于同一个锁，disable 后不能被迟到的 init/轮转重新打开。
        if !accepts(enabled(), LEVEL.load(Ordering::Relaxed), level) {
            return;
        }
        sink.attempt(Some(&text), Instant::now())
    };
    // stderr 故障不 panic，也不会阻止文件写入；不持文件状态锁等待 stderr。
    stderr(&text);
    emit_diagnostic(diagnostic);
}

/// 不读取配置、不触碰普通 logger 锁、不轮转。文件已满时放弃文件，保留 stderr 诊断。
fn emergency_append(path: &Path, text: &str, max_bytes: u64) -> io::Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    if file.metadata()?.len().saturating_add(text.len() as u64) > max_bytes {
        return Err(io::Error::other("panic 日志文件已满，跳过异常追加"));
    }
    file.write_all(text.as_bytes())
}

fn direct_line(msg: &str) {
    if !enabled() {
        return;
    }
    let text = record("PANIC", msg);
    stderr(&text);
    let Some(dir) = dir() else { return };
    if let Err(error) = emergency_append(&dir.join("jiadian.log"), &text, MAX_BYTES) {
        stderr(&record("WARN", &format!("panic 日志未落盘：{error}")));
    }
}

/// 记录普通 Rust panic，再调用原钩子；不改变 release panic=abort 或承诺恢复硬崩溃。
pub fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        direct_line(&format!("{info}"));
        default(info);
    }));
}

pub fn info(msg: &str) {
    line("INFO", msg);
}
pub fn warn(msg: &str) {
    line("WARN", msg);
}
pub fn error(msg: &str) {
    line("ERROR", msg);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::Arc;
    use std::sync::atomic::AtomicU64;

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Self {
            // 测试文件仅写入仓库自己的忽略构建目录，避免依赖宿主临时目录的授权。
            let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join("logging-test-tmp");
            fs::create_dir_all(&root).unwrap();
            let path = root.join(format!(
                "jiadian-log-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path)
                .unwrap_or_else(|e| panic!("无法创建隔离测试目录 {}：{e}", path.display()));
            Self(path)
        }
        fn path(&self) -> PathBuf {
            self.0.join("test.log")
        }
        fn sink(&self, max_bytes: u64) -> FileSink {
            FileSink {
                path: Some(self.path()),
                max_bytes,
                ..FileSink::new()
            }
        }
        fn backup(&self) -> PathBuf {
            self.0.join("test.log.1")
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn level_and_enabled_filter_preserve_existing_semantics() {
        for threshold in 0..=2 {
            for level in ["INFO", "WARN", "ERROR", "custom", "warn"] {
                assert!(!accepts(false, threshold, level));
                assert_eq!(accepts(true, threshold, level), rank(level) >= threshold);
            }
        }
        assert_eq!(level_rank(" Warning "), 1);
        assert_eq!(level_rank("ERROR"), 2);
        assert_eq!(level_rank("unknown"), 0);
        assert_eq!(level_rank(""), 0);
    }

    #[test]
    fn records_are_bounded_utf8_and_have_process_context() {
        let text = record("ERROR", &"中文🙂\n[ERROR]\r\0".repeat(4000));
        assert!(text.len() <= MAX_RECORD_BYTES);
        assert!(text.ends_with(&format!("{TRUNCATED}\n")));
        assert_eq!(text.matches('\n').count(), 1);
        assert!(text.contains("\\n[ERROR]\\r\\0"));
        assert!(text.contains(&format!("pid={}", std::process::id())));
        assert!(text.contains("tid=ThreadId("));
    }

    #[test]
    fn exact_capacity_fits_and_next_record_rotates_before_write() {
        let dir = TempDir::new();
        let mut sink = dir.sink(8);
        let now = Instant::now();
        assert!(sink.attempt(Some("1234567\n"), now).is_none());
        assert!(!dir.backup().exists());
        assert!(sink.attempt(Some("next\n"), now).is_none());
        assert_eq!(fs::read_to_string(dir.backup()).unwrap(), "1234567\n");
        assert_eq!(fs::read_to_string(dir.path()).unwrap(), "next\n");
    }

    #[test]
    fn oversized_startup_file_rotates_and_existing_backup_is_replaced() {
        let dir = TempDir::new();
        fs::write(dir.path(), "old-data-already-too-long\n").unwrap();
        fs::write(dir.backup(), "previous backup\n").unwrap();
        let mut sink = dir.sink(8);
        assert!(sink.attempt(Some("new\n"), Instant::now()).is_none());
        assert_eq!(
            fs::read_to_string(dir.backup()).unwrap(),
            "old-data-already-too-long\n"
        );
        assert_eq!(fs::read_to_string(dir.path()).unwrap(), "new\n");
    }

    #[test]
    fn rotation_failure_stops_growth_and_recovers_after_cooldown() {
        let dir = TempDir::new();
        fs::write(dir.path(), "1234567\n").unwrap();
        // 用一个目录阻塞 rename 的目标，不改 ACL，不接触真实日志。
        fs::create_dir(dir.backup()).unwrap();
        let mut sink = dir.sink(8);
        let now = Instant::now();
        assert!(
            sink.attempt(Some("next\n"), now)
                .unwrap()
                .1
                .contains("轮转日志")
        );
        assert!(sink.file.is_none());
        for _ in 0..1000 {
            assert!(
                sink.attempt(Some("next\n"), now + Duration::from_secs(1))
                    .is_none()
            );
        }
        assert_eq!(fs::metadata(dir.path()).unwrap().len(), 8);
        fs::remove_dir(dir.backup()).unwrap();
        let recovered = sink.attempt(Some("next\n"), now + RETRY_DELAY).unwrap();
        assert!(recovered.1.contains("输出恢复") && recovered.1.contains("1001"));
        assert_eq!(fs::read_to_string(dir.path()).unwrap(), "next\n");
    }

    #[test]
    fn failed_open_is_not_retried_for_every_record() {
        let dir = TempDir::new();
        fs::create_dir(dir.path()).unwrap();
        let mut sink = dir.sink(128);
        let now = Instant::now();
        assert!(
            sink.attempt(Some("first\n"), now)
                .unwrap()
                .1
                .contains("打开日志")
        );
        fs::remove_dir(dir.path()).unwrap();
        for _ in 0..1000 {
            assert!(
                sink.attempt(Some("skipped\n"), now + Duration::from_secs(1))
                    .is_none()
            );
        }
        assert!(!dir.path().exists(), "冷却内不应重新打开文件");
        assert!(
            sink.attempt(Some("recovered\n"), now + RETRY_DELAY)
                .unwrap()
                .1
                .contains("输出恢复")
        );
        assert_eq!(fs::read_to_string(dir.path()).unwrap(), "recovered\n");
    }

    #[test]
    fn failed_write_drops_handle_and_reopens_without_replaying_record() {
        let dir = TempDir::new();
        fs::write(dir.path(), "prefix\n").unwrap();
        let mut sink = dir.sink(128);
        sink.file = Some(File::open(dir.path()).unwrap()); // 故意注入只读句柄，可靠地制造写入错误。
        let now = Instant::now();
        assert!(
            sink.attempt(Some("failed\n"), now)
                .unwrap()
                .1
                .contains("追加日志失败")
        );
        assert!(sink.file.is_none());
        assert!(
            sink.attempt(Some("success\n"), now + RETRY_DELAY)
                .unwrap()
                .1
                .contains("输出恢复")
        );
        assert_eq!(fs::read_to_string(dir.path()).unwrap(), "prefix\nsuccess\n");
    }

    #[test]
    fn closing_sink_preserves_path_and_allows_lazy_reopen() {
        let dir = TempDir::new();
        let mut sink = dir.sink(128);
        sink.attempt(Some("before\n"), Instant::now());
        sink.close();
        assert!(sink.file.is_none());
        assert_eq!(sink.path, Some(dir.path()));
        assert!(sink.attempt(Some("after\n"), Instant::now()).is_none());
        assert_eq!(fs::read_to_string(dir.path()).unwrap(), "before\nafter\n");
    }

    #[test]
    fn concurrent_records_are_not_torn_or_duplicated() {
        let dir = TempDir::new();
        let sink = Arc::new(Mutex::new(dir.sink(MAX_BYTES)));
        let threads: Vec<_> = (0..8)
            .map(|thread| {
                let sink = sink.clone();
                std::thread::spawn(move || {
                    for seq in 0..40 {
                        let text = format!("线程={thread},序号={seq},中文🙂\n");
                        assert!(
                            lock_sink(&sink)
                                .attempt(Some(&text), Instant::now())
                                .is_none()
                        );
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        drop(sink);
        let text = fs::read_to_string(dir.path()).unwrap();
        let lines: HashSet<_> = text.lines().collect();
        assert_eq!(text.lines().count(), 320);
        assert_eq!(lines.len(), 320);
        for thread in 0..8 {
            for seq in 0..40 {
                assert!(lines.contains(format!("线程={thread},序号={seq},中文🙂").as_str()));
            }
        }
    }

    #[test]
    fn concurrent_rotation_keeps_complete_records_within_capacity() {
        let dir = TempDir::new();
        let sink = Arc::new(Mutex::new(dir.sink(128)));
        let threads: Vec<_> = (0..4)
            .map(|thread| {
                let sink = sink.clone();
                std::thread::spawn(move || {
                    for seq in 0..30 {
                        let text = format!("record={thread}:{seq:02}\n");
                        assert!(
                            lock_sink(&sink)
                                .attempt(Some(&text), Instant::now())
                                .is_none()
                        );
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        drop(sink);
        for path in [dir.path(), dir.backup()] {
            let text = fs::read_to_string(path).unwrap();
            assert!(text.len() <= 128 && text.ends_with('\n'));
            for line in text.lines() {
                let fields: Vec<_> = line.strip_prefix("record=").unwrap().split(':').collect();
                assert_eq!(fields.len(), 2);
                assert!(fields[0].parse::<usize>().unwrap() < 4);
                assert!(fields[1].parse::<usize>().unwrap() < 30);
            }
        }
    }

    #[test]
    fn poisoned_local_logger_recovers_without_a_second_panic() {
        let dir = TempDir::new();
        let sink = Arc::new(Mutex::new(dir.sink(128)));
        let other = sink.clone();
        assert!(
            std::thread::spawn(move || {
                let _guard = other.lock().unwrap();
                panic!("测试专用：中断日志状态操作");
            })
            .join()
            .is_err()
        );
        let mut guard = lock_sink(&sink);
        assert!(!sink.is_poisoned());
        assert!(
            guard
                .attempt(Some("recovered\n"), Instant::now())
                .unwrap()
                .1
                .contains("输出恢复")
        );
        assert_eq!(fs::read_to_string(dir.path()).unwrap(), "recovered\n");
    }

    #[test]
    fn stderr_io_error_does_not_panic_or_prevent_file_output() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> io::Result<()> {
                Err(io::ErrorKind::BrokenPipe.into())
            }
        }
        write_best_effort(&mut Broken, "test\n");
        let dir = TempDir::new();
        let mut sink = dir.sink(128);
        assert!(
            sink.attempt(Some("file still works\n"), Instant::now())
                .is_none()
        );
        assert_eq!(
            fs::read_to_string(dir.path()).unwrap(),
            "file still works\n"
        );
    }

    #[test]
    fn normal_capacity_reserves_one_bounded_panic_record() {
        let dir = TempDir::new();
        assert_eq!(FileSink::new().max_bytes, NORMAL_BYTES);
        fs::write(dir.path(), vec![b'x'; NORMAL_BYTES as usize]).unwrap();
        let text = record("PANIC", &"崩溃上下文🙂".repeat(MAX_RECORD_BYTES));
        assert!(text.len() <= MAX_RECORD_BYTES);
        emergency_append(&dir.path(), &text, MAX_BYTES).unwrap();
        assert!(fs::metadata(dir.path()).unwrap().len() <= MAX_BYTES);
    }

    #[test]
    fn emergency_path_does_not_take_regular_sink_lock_or_rotate() {
        let dir = TempDir::new();
        let sink = Mutex::new(dir.sink(8));
        let _held = sink.lock().unwrap();
        emergency_append(&dir.path(), "panic\n", 8).unwrap();
        assert!(emergency_append(&dir.path(), "again\n", 8).is_err());
        assert_eq!(fs::read_to_string(dir.path()).unwrap(), "panic\n");
        assert!(!dir.backup().exists());
    }
}
