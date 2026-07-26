// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors

//! ArknightsCostBarRuler 本地 API 客户端。
//!
//! 一个后台线程独占 WebSocket，把最新快照放进共享状态；调用方通过
//! [`FrameSource`] 读取。断线自动重连。

use std::{
    io,
    net::TcpStream,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Condvar, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use tungstenite::{stream::MaybeTlsStream, Message, WebSocket};

use crate::{
    command::{parse_inbound, Command, Envelope, Inbound},
    snapshot::{Snapshot, EXPECTED_API_VERSION},
    source::{ConnectionStatus, FrameError, FrameSource},
};

/// 尺子默认监听地址。上游写死在 `127.0.0.1:2606`，只绑本机。
pub const DEFAULT_WS_URL: &str = "ws://127.0.0.1:2606/";

/// socket 读超时。决定 worker 循环的最小转动周期。
const READ_TIMEOUT: Duration = Duration::from_millis(2);
/// 有等待者时主动 `getSnapshot` 的间隔。
const POLL_INTERVAL: Duration = Duration::from_millis(2);
/// 断线后的重连间隔。
const RECONNECT_DELAY: Duration = Duration::from_millis(800);

struct Shared {
    status: ConnectionStatus,
    latest: Option<Snapshot>,
    /// 正在 `wait_next` 里阻塞的调用方数量。>0 时 worker 主动轮询。
    waiters: usize,
    /// 每收到一条快照自增，供 condvar 判断"确实有更新"（防止虚假唤醒空转）。
    revision: u64,
    /// 最近一次尺子返回的错误，供 UI 展示。
    last_error: Option<(String, String)>,
}

struct Inner {
    shared: Mutex<Shared>,
    cv: Condvar,
    stop: AtomicBool,
}

impl Inner {
    fn set_status(&self, status: ConnectionStatus) {
        let mut guard = self.shared.lock().expect("shared state poisoned");
        if guard.status != status {
            log::info!("ruler connection: {:?} -> {status:?}", guard.status);
            guard.status = status;
            if !status.is_connected() {
                // 断线后旧快照不再代表现实，清掉以免状态机拿着陈旧帧数继续跑。
                guard.latest = None;
            }
            guard.revision = guard.revision.wrapping_add(1);
            self.cv.notify_all();
        }
    }

    fn publish(&self, snapshot: Snapshot) {
        let mut guard = self.shared.lock().expect("shared state poisoned");
        guard.latest = Some(snapshot);
        guard.revision = guard.revision.wrapping_add(1);
        self.cv.notify_all();
    }

    fn record_error(&self, code: String, message: String) {
        let mut guard = self.shared.lock().expect("shared state poisoned");
        guard.last_error = Some((code, message));
    }

    fn waiters(&self) -> usize {
        self.shared.lock().expect("shared state poisoned").waiters
    }
}

/// 尺子客户端。`Drop` 时会让后台线程退出。
pub struct RulerClient {
    inner: Arc<Inner>,
    outbox: mpsc::Sender<String>,
    worker: Option<thread::JoinHandle<()>>,
}

impl RulerClient {
    /// 连接默认地址。构造立即返回，连接在后台建立。
    pub fn connect_default() -> Self {
        Self::connect(DEFAULT_WS_URL)
    }

    pub fn connect(url: impl Into<String>) -> Self {
        let url = url.into();
        let inner = Arc::new(Inner {
            shared: Mutex::new(Shared {
                status: ConnectionStatus::Disconnected,
                latest: None,
                waiters: 0,
                revision: 0,
                last_error: None,
            }),
            cv: Condvar::new(),
            stop: AtomicBool::new(false),
        });
        let (outbox, inbox) = mpsc::channel::<String>();
        let worker = {
            let inner = Arc::clone(&inner);
            thread::Builder::new()
                .name("ruler-ws".into())
                .spawn(move || worker_loop(&url, &inner, &inbox))
                .expect("failed to spawn ruler worker thread")
        };
        Self {
            inner,
            outbox,
            worker: Some(worker),
        }
    }

    /// 最近一次尺子返回的错误信封。
    pub fn last_error(&self) -> Option<(String, String)> {
        self.inner
            .shared
            .lock()
            .expect("shared state poisoned")
            .last_error
            .clone()
    }

    /// 阻塞等待连上尺子并拿到第一条快照。
    ///
    /// 用于首次运行向导 / 探针工具里的"尺子在不在"检测。
    ///
    /// 刻意**不用** API 文档里的 HTTP 快照接口：尺子的极简 HTTP 服务在关闭连接时，
    /// 如果接收缓冲区里还有客户端发来的、它没读完的请求头，Windows 会发 RST 而不是
    /// FIN，把已经写出的响应一起冲掉 —— 客户端就看到 `os error 10054`。
    /// curl 因为请求头短、读得快通常侥幸成功，ureq / .NET 则稳定失败。
    /// WebSocket 通道没有这个问题，而且本来就是实际的数据通道，没必要维护第二条路。
    pub fn probe(&self, timeout: Duration) -> Result<Snapshot, FrameError> {
        if let Some(snapshot) = self.latest() {
            return Ok(snapshot);
        }
        self.wait_next(0, timeout)
    }
}

impl Drop for RulerClient {
    fn drop(&mut self) {
        self.inner.stop.store(true, Ordering::Relaxed);
        // 唤醒可能正在 condvar 上等待的调用方。
        self.inner.cv.notify_all();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl FrameSource for RulerClient {
    fn status(&self) -> ConnectionStatus {
        self.inner
            .shared
            .lock()
            .expect("shared state poisoned")
            .status
    }

    fn latest(&self) -> Option<Snapshot> {
        self.inner
            .shared
            .lock()
            .expect("shared state poisoned")
            .latest
            .clone()
    }

    fn wait_next(&self, after_frame_id: u64, timeout: Duration) -> Result<Snapshot, FrameError> {
        let deadline = Instant::now() + timeout;
        let mut guard = self.inner.shared.lock().expect("shared state poisoned");

        // 声明"我在等" —— worker 看到 waiters > 0 才会主动轮询 getSnapshot。
        // 下面每条 return 路径都必须把它减回去，所以统一在函数尾部处理。
        guard.waiters += 1;
        let outcome = loop {
            if self.inner.stop.load(Ordering::Relaxed) {
                break Err(FrameError::NotConnected);
            }
            if let Some(s) = guard.latest.as_ref() {
                if s.frame_id.is_some_and(|id| id > after_frame_id) {
                    break Ok(s.clone());
                }
            }
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                break Err(FrameError::Timeout(timeout));
            };
            let (next, wait) = self
                .inner
                .cv
                .wait_timeout(guard, remaining)
                .expect("shared state poisoned");
            guard = next;
            if wait.timed_out() && Instant::now() >= deadline {
                break Err(FrameError::Timeout(timeout));
            }
        };
        guard.waiters -= 1;
        outcome
    }

    fn send(&self, command: Command) -> Result<(), FrameError> {
        if !self.status().is_connected() {
            return Err(FrameError::NotConnected);
        }
        self.outbox
            .send(command.to_json(None))
            .map_err(|e| FrameError::Send(e.to_string()))
    }
}

// ————————————————————————————————————————————————————————————————
// worker
// ————————————————————————————————————————————————————————————————

fn worker_loop(url: &str, inner: &Arc<Inner>, inbox: &mpsc::Receiver<String>) {
    while !inner.stop.load(Ordering::Relaxed) {
        inner.set_status(ConnectionStatus::Connecting);
        match tungstenite::connect(url) {
            Ok((mut socket, response)) => {
                log::info!("connected to ruler at {url} (HTTP {})", response.status());
                if let Err(e) = set_read_timeout(&mut socket, READ_TIMEOUT) {
                    log::warn!("could not set ruler socket read timeout: {e}; giving up on this connection");
                    let _ = socket.close(None);
                    inner.set_status(ConnectionStatus::Disconnected);
                    sleep_unless_stopped(inner, RECONNECT_DELAY);
                    continue;
                }
                inner.set_status(ConnectionStatus::Connected);
                // 连上先要一次快照，免得 UI 在游戏没动静时一直是空的。
                let _ = socket.send(Message::text(Command::GetSnapshot.to_json(None)));
                pump(&mut socket, inner, inbox);
                let _ = socket.close(None);
            }
            Err(e) => {
                // 尺子没开是常态，不值得每次都刷 warn。
                log::debug!("ruler connect failed: {e}");
            }
        }
        inner.set_status(ConnectionStatus::Disconnected);
        // 丢弃断线期间积压的命令，避免重连后一次性喷给尺子。
        while inbox.try_recv().is_ok() {}
        sleep_unless_stopped(inner, RECONNECT_DELAY);
    }
    inner.set_status(ConnectionStatus::Disconnected);
}

/// 单个连接的收发循环。返回即表示该连接已不可用。
fn pump(
    socket: &mut WebSocket<MaybeTlsStream<TcpStream>>,
    inner: &Arc<Inner>,
    inbox: &mpsc::Receiver<String>,
) {
    let mut last_poll = Instant::now() - POLL_INTERVAL;
    loop {
        if inner.stop.load(Ordering::Relaxed) {
            return;
        }

        // 1. 把调用方排队的命令发出去
        let mut wrote = false;
        while let Ok(text) = inbox.try_recv() {
            if let Err(e) = socket.write(Message::text(text)) {
                log::warn!("ruler write failed: {e}");
                return;
            }
            wrote = true;
        }

        // 2. 有人在 wait_next 里等 => 主动轮询。
        //    尺子只在状态变化时推送，暂停期间不会自己推，不轮询就会一直等到超时。
        if inner.waiters() > 0 && last_poll.elapsed() >= POLL_INTERVAL {
            last_poll = Instant::now();
            if let Err(e) = socket.write(Message::text(Command::GetSnapshot.to_json(None))) {
                log::warn!("ruler poll write failed: {e}");
                return;
            }
            wrote = true;
        }

        if wrote {
            match socket.flush() {
                Ok(()) => {}
                Err(e) if is_would_block(&e) => {}
                Err(e) => {
                    log::warn!("ruler flush failed: {e}");
                    return;
                }
            }
        }

        // 3. 读一条消息（带 READ_TIMEOUT，所以最多阻塞几毫秒）
        match socket.read() {
            Ok(Message::Text(text)) => handle_text(inner, &text),
            Ok(Message::Binary(_)) => log::debug!("ignoring binary frame from ruler"),
            Ok(Message::Close(_)) => {
                log::info!("ruler closed the connection");
                return;
            }
            // Ping/Pong/Frame 由 tungstenite 内部处理，这里无事可做。
            Ok(_) => {}
            Err(e) if is_would_block(&e) => {}
            Err(e) => {
                log::info!("ruler read ended: {e}");
                return;
            }
        }
    }
}

fn handle_text(inner: &Arc<Inner>, text: &str) {
    let Some(inbound) = parse_inbound(text) else {
        log::debug!("unparsable message from ruler: {text}");
        return;
    };
    let payload = match inbound {
        Inbound::Push(value) => value,
        Inbound::Reply(Envelope::Snapshot { payload, .. }) => payload,
        Inbound::Reply(Envelope::Error {
            code,
            message,
            request_id,
        }) => {
            log::warn!("ruler error [{code}] for {request_id:?}: {message}");
            inner.record_error(code, message);
            return;
        }
        Inbound::Reply(Envelope::Ack { action, .. }) => {
            log::debug!("ruler acked {action}");
            return;
        }
        // 本项目目前不用 getFrame 的历史帧查询。
        Inbound::Reply(Envelope::Frame { .. }) => return,
    };

    match serde_json::from_value::<Snapshot>(payload) {
        Ok(snapshot) => {
            warn_once_on_api_version(snapshot.api_version);
            inner.publish(snapshot);
        }
        Err(e) => log::warn!("could not decode ruler snapshot: {e}"),
    }
}

fn warn_once_on_api_version(version: u32) {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    if version != 0 && version != EXPECTED_API_VERSION {
        ONCE.call_once(|| {
            log::warn!(
                "ruler speaks apiVersion {version}, this client targets {EXPECTED_API_VERSION}; \
                 upstream promises additive-only changes so continuing anyway"
            );
        });
    }
}

fn set_read_timeout(
    socket: &mut WebSocket<MaybeTlsStream<TcpStream>>,
    timeout: Duration,
) -> io::Result<()> {
    match socket.get_ref() {
        MaybeTlsStream::Plain(stream) => stream.set_read_timeout(Some(timeout)),
        // 只连 127.0.0.1 的明文端口，理论上到不了这里。
        _ => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "ruler connection is not a plain TCP stream",
        )),
    }
}

/// 读超时在不同平台上表现为 `WouldBlock` 或 `TimedOut`，都不是真错误。
fn is_would_block(error: &tungstenite::Error) -> bool {
    matches!(
        error,
        tungstenite::Error::Io(e)
            if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut
    )
}

fn sleep_unless_stopped(inner: &Arc<Inner>, total: Duration) {
    const SLICE: Duration = Duration::from_millis(50);
    let deadline = Instant::now() + total;
    while Instant::now() < deadline {
        if inner.stop.load(Ordering::Relaxed) {
            return;
        }
        thread::sleep(SLICE.min(deadline - Instant::now()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disconnected_client_reports_not_connected() {
        // 没有尺子在跑时，构造不应 panic，send 应干净地失败。
        let client = RulerClient::connect("ws://127.0.0.1:1/");
        assert!(matches!(
            client.send(Command::GetSnapshot),
            Err(FrameError::NotConnected)
        ));
        assert!(client.latest().is_none());
    }

    #[test]
    fn wait_next_times_out_without_a_server() {
        let client = RulerClient::connect("ws://127.0.0.1:1/");
        let started = Instant::now();
        let result = client.wait_next(0, Duration::from_millis(120));
        assert!(matches!(result, Err(FrameError::Timeout(_))));
        // 必须真的等够时间再超时，不能立刻返回（那说明 condvar 逻辑写反了）。
        assert!(started.elapsed() >= Duration::from_millis(100));
    }

    #[test]
    fn would_block_is_not_a_real_error() {
        let wb = tungstenite::Error::Io(io::Error::new(io::ErrorKind::WouldBlock, "x"));
        let to = tungstenite::Error::Io(io::Error::new(io::ErrorKind::TimedOut, "x"));
        let real = tungstenite::Error::Io(io::Error::new(io::ErrorKind::ConnectionReset, "x"));
        assert!(is_would_block(&wb));
        assert!(is_would_block(&to));
        assert!(!is_would_block(&real));
    }
}
