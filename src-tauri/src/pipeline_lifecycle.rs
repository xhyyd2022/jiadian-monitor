//! Hardware-free lifecycle authority. Locks only protect small in-memory records;
//! no caller may perform I/O, logging, or waits while holding them.
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

pub const MAX_REQUEST_ID_BYTES: usize = 128;
const HISTORY_LIMIT: usize = 256;
const DONE_LIMIT: usize = 1024;
const MAX_SESSION_ID: u64 = (1u64 << 53) - 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase { Idle, Starting, Running, Stopping }
impl Phase {
    pub fn as_str(self) -> &'static str {
        match self { Self::Idle => "idle", Self::Starting => "starting", Self::Running => "running", Self::Stopping => "stopping" }
    }
}

#[derive(Clone)]
pub struct Session {
    pub id: u64,
    pub request_id: Option<String>,
    pub cancel: Arc<AtomicBool>,
}
impl Session {
    pub fn check(&self) -> Result<(), String> {
        if self.cancel.load(Ordering::SeqCst) { Err("启动请求已取消".into()) } else { Ok(()) }
    }
}
struct Entry { session: Session, phase: Phase, ready: bool }
impl Entry {
    fn stop(&mut self) {
        self.phase = Phase::Stopping;
        self.session.cancel.store(true, Ordering::SeqCst);
    }
}
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub session_id: Option<u64>,
    pub request_id: Option<String>,
    pub phase: Phase,
    pub active: bool,
    pub ready: bool,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Operation { Start(u64), Release(u64) }
struct Inner {
    next_id: u64,
    current: Option<Entry>,
    pending: Option<Entry>,
    operation: Option<Operation>,
    // Unknown cancellation must survive registration arriving later. Completed
    // requests are diagnostics only and must never cancel another active ID.
    history: VecDeque<String>,
    recent_done: VecDeque<String>,
}
impl Inner {
    fn remember(&mut self, id: &str) {
        if !self.history.iter().any(|s| s == id) {
            if self.history.len() == HISTORY_LIMIT { self.history.pop_front(); }
            self.history.push_back(id.to_owned());
        }
    }
    fn remember_done(&mut self, id: &str) {
        if !self.recent_done.iter().any(|s| s == id) {
            if self.recent_done.len() == DONE_LIMIT { self.recent_done.pop_front(); }
            self.recent_done.push_back(id.to_owned());
        }
    }
    fn allocate(&mut self) -> Result<u64, String> {
        if self.next_id >= MAX_SESSION_ID { return Err("管线会话 ID 已耗尽，请重启应用".into()); }
        self.next_id += 1;
        Ok(self.next_id)
    }
}

pub struct Lifecycle { inner: Mutex<Inner> }
impl Lifecycle {
    pub fn new() -> Self {
        Self { inner: Mutex::new(Inner { next_id: 0, current: None, pending: None, operation: None, history: VecDeque::new(), recent_done: VecDeque::new() }) }
    }
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn validate_request_id(id: &str) -> Result<(), String> {
        if id.is_empty() || id.len() > MAX_REQUEST_ID_BYTES {
            Err(format!("requestId 长度必须为 1..={MAX_REQUEST_ID_BYTES} 字节"))
        } else { Ok(()) }
    }
    pub fn prepare_start(self: &Arc<Self>, request_id: Option<String>) -> Result<StartTicket, String> {
        if let Some(id) = request_id.as_deref() { Self::validate_request_id(id)?; }
        let mut g = self.lock();
        if g.operation.is_some() { return Err("管线启停操作忙，请等待当前操作收尾".into()); }
        if let Some(id) = request_id.as_deref() {
            if g.history.iter().any(|s| s == id) || g.recent_done.iter().any(|s| s == id)
                || g.current.as_ref().is_some_and(|e| e.session.request_id.as_deref() == Some(id)) {
                return Err("requestId 已取消或已使用，不可复用".into());
            }
        }
        let id = g.allocate()?;
        let session = Session { id, request_id, cancel: Arc::new(AtomicBool::new(false)) };
        if let Some(old) = g.current.as_mut() { old.stop(); }
        g.pending = Some(Entry { session: session.clone(), phase: Phase::Starting, ready: false });
        g.operation = Some(Operation::Start(id));
        Ok(StartTicket { registry: self.clone(), session, activated: false })
    }
    pub fn prepare_release(self: &Arc<Self>) -> Result<ReleaseTicket, String> {
        let mut g = self.lock();
        // Even a busy release has an immediate, identity-bound cancellation
        // effect. Do not recursively call stop() while holding this mutex.
        if let Some(e) = g.current.as_mut() { e.stop(); }
        if let Some(e) = g.pending.as_mut() { e.stop(); }
        if g.operation.is_some() { return Err("管线启停操作忙，请等待当前操作收尾".into()); }
        let id = g.allocate()?;
        let target = g.current.as_ref().or(g.pending.as_ref()).map(|e| e.session.id);
        g.operation = Some(Operation::Release(id));
        Ok(ReleaseTicket { registry: self.clone(), operation_id: id, target })
    }
    pub fn cancel_request(&self, id: &str) -> Result<(bool, bool), String> {
        Self::validate_request_id(id)?;
        let mut g = self.lock();
        // Exact current/pending identity is the only authority to cancel. Never
        // interpret an unknown/old ID as a request to stop the newest session.
        let mut known = false;
        if let Some(e) = g.current.as_mut() {
            if e.session.request_id.as_deref() == Some(id) { e.stop(); known = true; }
        }
        if let Some(e) = g.pending.as_mut() {
            if e.session.request_id.as_deref() == Some(id) { e.stop(); known = true; }
        }
        let stale = !known && g.recent_done.iter().any(|s| s == id);
        if !known && !stale { g.remember(id); }
        Ok((known, stale))
    }
    pub fn stop_session(&self, id: u64) {
        let mut g = self.lock();
        if let Some(e) = g.current.as_mut().filter(|e| e.session.id == id) { e.stop(); }
        if let Some(e) = g.pending.as_mut().filter(|e| e.session.id == id) { e.stop(); }
    }
    pub fn stop(&self) {
        let mut g = self.lock();
        if let Some(e) = g.current.as_mut() { e.stop(); }
        if let Some(e) = g.pending.as_mut() { e.stop(); }
    }
    pub fn snapshot(&self) -> Snapshot {
        let g = self.lock();
        let entry = g.current.as_ref().or(g.pending.as_ref());
        let releasing = matches!(g.operation, Some(Operation::Release(_)));
        Snapshot {
            session_id: entry.map(|e| e.session.id),
            request_id: entry.and_then(|e| e.session.request_id.clone()),
            phase: if releasing { Phase::Stopping } else { entry.map(|e| e.phase).unwrap_or(if g.operation.is_some() { Phase::Starting } else { Phase::Idle }) },
            active: entry.is_some() || g.operation.is_some(),
            ready: !releasing && entry.map(|e| e.ready && e.phase == Phase::Running && !e.session.cancel.load(Ordering::SeqCst)).unwrap_or(false),
        }
    }
    pub fn finished(&self, id: u64) -> bool {
        let g = self.lock();
        !g.current.as_ref().is_some_and(|e| e.session.id == id)
            && !g.pending.as_ref().is_some_and(|e| e.session.id == id)
    }
    pub fn accepts_preview(&self, id: u64) -> bool {
        let g = self.lock();
        g.current.as_ref().is_some_and(|e| e.session.id == id && e.phase == Phase::Running && !e.session.cancel.load(Ordering::SeqCst))
    }
    pub fn mark_running(&self, id: u64) -> bool {
        let mut g = self.lock();
        if let Some(e) = g.current.as_mut().filter(|e| e.session.id == id) {
            if !e.session.cancel.load(Ordering::SeqCst) {
                e.phase = Phase::Running;
                e.ready = true;
                return true;
            }
        }
        false
    }
    fn finish(&self, id: u64) {
        let mut g = self.lock();
        if g.current.as_ref().is_some_and(|e| e.session.id == id) {
            if let Some(e) = g.current.take() {
                if let Some(request_id) = e.session.request_id { g.remember_done(&request_id); }
            }
        }
        if g.pending.as_ref().is_some_and(|e| e.session.id == id) {
            if let Some(e) = g.pending.take() {
                if let Some(request_id) = e.session.request_id { g.remember_done(&request_id); }
            }
        }
    }
}

// Owned Send tickets preserve fail-fast admission across spawn_blocking queues.
pub struct StartTicket { registry: Arc<Lifecycle>, pub session: Session, activated: bool }
impl StartTicket {
    pub fn previous_finished(&self) -> bool { self.registry.lock().current.is_none() }
    pub fn activate(&mut self) -> Result<SessionOwner, String> {
        let mut g = self.registry.lock();
        self.session.check()?;
        if self.activated || g.operation != Some(Operation::Start(self.session.id)) {
            return Err("启动操作已失效".into());
        }
        if g.current.is_some() { return Err("上一会话仍在收尾，禁止重叠启动".into()); }
        let e = g.pending.take().filter(|e| e.session.id == self.session.id).ok_or("启动请求已失效")?;
        g.current = Some(e);
        self.activated = true;
        Ok(SessionOwner { registry: self.registry.clone(), session: self.session.clone() })
    }
}
impl Drop for StartTicket {
    fn drop(&mut self) {
        let mut g = self.registry.lock();
        if !self.activated && g.pending.as_ref().is_some_and(|e| e.session.id == self.session.id) {
            if let Some(e) = g.pending.take() {
                if let Some(request_id) = e.session.request_id { g.remember_done(&request_id); }
            }
        }
        if g.operation == Some(Operation::Start(self.session.id)) { g.operation = None; }
    }
}

pub struct SessionOwner { registry: Arc<Lifecycle>, pub session: Session }
impl Drop for SessionOwner { fn drop(&mut self) { self.registry.finish(self.session.id); } }

pub struct ReleaseTicket { registry: Arc<Lifecycle>, operation_id: u64, pub target: Option<u64> }
impl ReleaseTicket {
    pub fn finished(&self) -> bool { self.target.map(|id| self.registry.finished(id)).unwrap_or(true) }
}
impl Drop for ReleaseTicket {
    fn drop(&mut self) {
        let mut g = self.registry.lock();
        if g.operation == Some(Operation::Release(self.operation_id)) { g.operation = None; }
    }
}

/// GUI transport only; capture/CLI callbacks retain the original PNG bytes.
pub fn preview_packet(session_id: u64, png: &[u8]) -> Vec<u8> {
    let mut packet = Vec::with_capacity(12 + png.len());
    packet.extend_from_slice(b"JDP1");
    packet.extend_from_slice(&session_id.to_le_bytes());
    packet.extend_from_slice(png);
    packet
}

#[cfg(test)]
mod tests {
    use super::*;
    fn registry() -> Arc<Lifecycle> { Arc::new(Lifecycle::new()) }
    fn running(r: &Arc<Lifecycle>, id: &str) -> SessionOwner {
        let mut ticket = r.prepare_start(Some(id.into())).unwrap();
        let owner = ticket.activate().unwrap();
        assert!(r.mark_running(owner.session.id));
        owner
    }
    #[test]
    fn old_completion_cannot_modify_new_session_or_ready() {
        let r = registry();
        let old = running(&r, "old");
        let old_id = old.session.id;
        let mut next = r.prepare_start(Some("new".into())).unwrap();
        assert_eq!(r.snapshot().phase, Phase::Stopping);
        assert!(next.activate().is_err());
        drop(old);
        let new = next.activate().unwrap();
        assert!(r.mark_running(new.session.id));
        r.finish(old_id);
        assert!(!r.mark_running(old_id));
        assert_eq!(r.snapshot().session_id, Some(new.session.id));
        assert!(r.snapshot().ready);
    }
    #[test]
    fn cancellation_before_registration_and_invalid_ids() {
        let r = registry();
        r.cancel_request("early").unwrap();
        assert!(r.prepare_start(Some("early".into())).is_err());
        assert!(r.cancel_request("").is_err());
        assert!(r.cancel_request(&"x".repeat(MAX_REQUEST_ID_BYTES + 1)).is_err());
        assert!(!r.snapshot().active);
    }
    #[test]
    fn cancellation_during_queued_and_active_start_is_not_idle_until_drop() {
        let r = registry();
        let mut queued = r.prepare_start(Some("queued".into())).unwrap();
        r.cancel_request("queued").unwrap();
        assert!(queued.activate().is_err());
        assert_eq!(r.snapshot().phase, Phase::Stopping);
        assert!(r.snapshot().active);
        drop(queued);
        let mut start = r.prepare_start(Some("active-start".into())).unwrap();
        let owner = start.activate().unwrap();
        r.cancel_request("active-start").unwrap();
        assert!(owner.session.check().is_err());
        assert!(!r.mark_running(owner.session.id));
        drop(start);
        assert_eq!(r.snapshot().phase, Phase::Stopping);
        drop(owner);
        assert_eq!(r.snapshot().phase, Phase::Idle);
    }
    #[test]
    fn repeated_old_cancel_never_cancels_new_request() {
        let r = registry();
        let old = running(&r, "old");
        r.cancel_request("old").unwrap();
        assert!(old.session.check().is_err());
        drop(old);
        let new = running(&r, "new");
        r.cancel_request("old").unwrap();
        r.cancel_request("unrelated").unwrap();
        assert!(new.session.check().is_ok());
        assert_eq!(r.snapshot().phase, Phase::Running);
        assert!(r.prepare_start(Some("old".into())).is_err());
    }
    #[test]
    fn admission_is_fail_fast_and_unfinished_worker_blocks_activation() {
        let r = registry();
        let worker = running(&r, "worker");
        let mut replacement = r.prepare_start(Some("replacement".into())).unwrap();
        assert!(r.prepare_start(Some("queued-stale".into())).is_err());
        assert!(r.prepare_release().is_err());
        assert!(replacement.activate().is_err());
        drop(replacement);
        assert_eq!(r.snapshot().phase, Phase::Stopping);
        assert!(r.snapshot().active);
        let mut retry = r.prepare_start(Some("retry".into())).unwrap();
        assert!(retry.activate().is_err());
        drop(worker);
        assert!(retry.activate().is_ok());
    }
    #[test]
    fn release_keeps_admission_and_stopping_even_after_worker_completed() {
        let r = registry();
        let worker = running(&r, "worker");
        let release = r.prepare_release().unwrap();
        assert!(!release.finished());
        drop(worker);
        assert!(release.finished());
        assert_eq!(r.snapshot().phase, Phase::Stopping);
        assert!(r.snapshot().active);
        assert!(r.prepare_start(None).is_err());
        drop(release);
        assert_eq!(r.snapshot().phase, Phase::Idle);
    }
    #[test]
    fn stale_preview_is_rejected_and_packet_is_little_endian() {
        let r = registry();
        let old = running(&r, "old");
        let old_id = old.session.id;
        assert!(r.accepts_preview(old_id));
        r.cancel_request("old").unwrap();
        assert!(!r.accepts_preview(old_id));
        drop(old);
        let new = running(&r, "new");
        assert!(!r.accepts_preview(old_id));
        assert!(r.accepts_preview(new.session.id));
        let packet = preview_packet(new.session.id, b"PNG");
        assert_eq!(&packet[..4], b"JDP1");
        assert_eq!(&packet[4..12], &new.session.id.to_le_bytes());
        assert_eq!(&packet[12..], b"PNG");
    }
    #[test]
    fn tombstones_are_bounded_and_session_ids_never_wrap() {
        let r = registry();
        for n in 0..HISTORY_LIMIT + 20 { r.cancel_request(&format!("cancel-{n}")).unwrap(); }
        assert_eq!(r.lock().history.len(), HISTORY_LIMIT);
        r.lock().next_id = MAX_SESSION_ID - 1;
        let last = r.prepare_start(None).unwrap();
        assert_eq!(last.session.id, MAX_SESSION_ID);
        drop(last);
        assert!(r.prepare_start(None).is_err());
        assert!(r.prepare_release().is_err());
    }
    #[test]
    fn busy_release_cancels_pending_without_stealing_admission() {
        let r = registry();
        let mut pending = r.prepare_start(Some("pending".into())).unwrap();
        let id = pending.session.id;
        assert!(r.prepare_release().is_err());
        assert!(pending.activate().is_err());
        assert!(!r.finished(id));
        assert_eq!(r.snapshot().phase, Phase::Stopping);
        assert!(r.prepare_start(Some("other".into())).is_err());
        drop(pending);
        assert!(r.finished(id));
        assert_eq!(r.snapshot().phase, Phase::Idle);
        assert!(!r.snapshot().ready);
    }
    #[test]
    fn stale_done_and_evicted_unknown_cancel_never_touch_unrelated_worker() {
        let r = registry();
        drop(running(&r, "done"));
        let worker = running(&r, "new");
        assert_eq!(r.cancel_request("done").unwrap(), (false, true));
        assert!(!r.lock().history.iter().any(|id| id == "done"));
        assert!(worker.session.check().is_ok());
        drop(worker);
        // Fill and evict recent_done using local, hardware-free sessions.
        for n in 0..DONE_LIMIT { drop(running(&r, &format!("completed-{n}"))); }
        let newest = running(&r, "newest");
        assert_eq!(r.cancel_request("done").unwrap(), (false, false));
        for n in 0..HISTORY_LIMIT { r.cancel_request(&format!("early-{n}")).unwrap(); }
        assert_eq!(r.cancel_request("done").unwrap(), (false, false));
        assert!(newest.session.check().is_ok());
        assert_eq!(r.snapshot().phase, Phase::Running);
        assert_eq!(r.lock().recent_done.len(), DONE_LIMIT);
    }
    #[test]
    fn dropping_unexecuted_tickets_releases_admission_and_keeps_idle_unready() {
        let r = registry();
        assert!(!r.snapshot().ready);
        let pending = r.prepare_start(Some("not-executed".into())).unwrap();
        assert!(r.snapshot().active);
        drop(pending);
        assert_eq!(r.snapshot().phase, Phase::Idle);
        assert!(!r.snapshot().active);
        assert!(!r.snapshot().ready);
        let release = r.prepare_release().unwrap();
        assert!(release.finished());
        assert_eq!(r.snapshot().phase, Phase::Stopping);
        assert!(r.prepare_start(None).is_err());
        drop(release);
        assert_eq!(r.snapshot().phase, Phase::Idle);
    }
    #[test]
    fn tickets_and_worker_owner_are_send() {
        fn assert_send<T: Send>() {}
        assert_send::<StartTicket>();
        assert_send::<ReleaseTicket>();
        assert_send::<SessionOwner>();
    }
}
