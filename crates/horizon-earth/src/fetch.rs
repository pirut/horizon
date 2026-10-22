//! Downloads, the disk cache and the in-memory caches every Earth source shares.
//!
//! Every source reads the same way: memory, then the disk cache, then the network, with one load
//! in flight per piece however many workers ask for it at once. The disk layout is Earth
//! Platform's (`terrarium-v1/z/x/y.png`, `esa-worldcover-v200/<file>.header`, ...), so a cache
//! folder Earth Platform filled is read as it is.
use std::{
    collections::HashMap,
    hash::Hash,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

/// Why a piece of Earth data could not be had.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Missing {
    /// The source has no such piece (HTTP 403 or 404): open ocean has no land cover files.
    Absent,
    /// The piece exists but could not be read now (network down, offline, corrupt).
    Failed(String),
}
impl std::fmt::Display for Missing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Absent => f.write_str("not available from the source"),
            Self::Failed(why) => f.write_str(why),
        }
    }
}
impl std::error::Error for Missing {}

/// Bytes and time spent on downloads, for the benchmark's report.
#[derive(Default)]
pub struct Totals {
    pub files: AtomicU64,
    pub bytes: AtomicU64,
    pub nanos: AtomicU64,
    pub disk_hits: AtomicU64,
}
impl Totals {
    pub fn snapshot(&self) -> (u64, u64, Duration, u64) {
        (
            self.files.load(Ordering::Relaxed),
            self.bytes.load(Ordering::Relaxed),
            Duration::from_nanos(self.nanos.load(Ordering::Relaxed)),
            self.disk_hits.load(Ordering::Relaxed),
        )
    }
}

/// HTTPS with kept-alive connections, a cap on transfers at once, and retries.
pub struct Http {
    agent: ureq::Agent,
    permits: Mutex<usize>,
    freed: Condvar,
    pub totals: Totals,
    allow_network: bool,
}
const ATTEMPTS: u32 = 3;
impl Http {
    pub fn new(max_transfers: usize, timeout: Duration, allow_network: bool) -> Self {
        let mut builder = ureq::AgentBuilder::new()
            .timeout_connect(timeout)
            .timeout(timeout * 3)
            .max_idle_connections_per_host(max_transfers.max(1))
            .user_agent("horizon-earth/0.1");
        if let Some(proxy) = ["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"]
            .iter()
            .find_map(|name| std::env::var(name).ok().filter(|v| !v.is_empty()))
            .and_then(|url| ureq::Proxy::new(url).ok())
        {
            builder = builder.proxy(proxy);
        }
        Self {
            agent: builder.build(),
            permits: Mutex::new(max_transfers.max(1)),
            freed: Condvar::new(),
            totals: Totals::default(),
            allow_network,
        }
    }
    fn acquire(&self) {
        let mut free = self.permits.lock().unwrap();
        while *free == 0 {
            free = self.freed.wait(free).unwrap();
        }
        *free -= 1;
    }
    fn release(&self) {
        *self.permits.lock().unwrap() += 1;
        self.freed.notify_one();
    }
    /// The whole resource, or `range` (first byte, length) of it.
    pub fn get(
        &self,
        url: &str,
        range: Option<(u64, u64)>,
        max: usize,
    ) -> Result<Vec<u8>, Missing> {
        if !self.allow_network {
            return Err(Missing::Failed(format!(
                "{url}: not cached and the network is off"
            )));
        }
        let mut last = String::new();
        for attempt in 0..ATTEMPTS {
            if attempt > 0 {
                std::thread::sleep(Duration::from_millis(250 << attempt));
            }
            self.acquire();
            let start = Instant::now();
            let mut request = self.agent.get(url);
            if let Some((first, len)) = range {
                request = request.set(
                    "Range",
                    &format!("bytes={first}-{}", first + len.max(1) - 1),
                );
            }
            let result = request.call();
            let outcome = match result {
                Ok(response) => {
                    let mut body = Vec::new();
                    match response
                        .into_reader()
                        .take(max as u64 + 1)
                        .read_to_end(&mut body)
                    {
                        Ok(_) if body.len() > max => Err(Err(Missing::Failed(format!(
                            "{url}: larger than {max} bytes"
                        )))),
                        Ok(_) => Ok(body),
                        Err(e) => Err(Ok(format!("{url}: {e}"))),
                    }
                }
                Err(ureq::Error::Status(403 | 404, _)) => Err(Err(Missing::Absent)),
                Err(ureq::Error::Status(code, _)) => Err(Ok(format!("{url}: HTTP {code}"))),
                Err(e) => Err(Ok(format!("{url}: {e}"))),
            };
            self.release();
            match outcome {
                Ok(body) => {
                    self.totals.files.fetch_add(1, Ordering::Relaxed);
                    self.totals
                        .bytes
                        .fetch_add(body.len() as u64, Ordering::Relaxed);
                    self.totals
                        .nanos
                        .fetch_add(start.elapsed().as_nanos() as u64, Ordering::Relaxed);
                    return Ok(body);
                }
                Err(Err(missing)) => return Err(missing),
                Err(Ok(why)) => last = why,
            }
        }
        Err(Missing::Failed(last))
    }
}

/// Earth data on disk, written atomically so a reader never sees half a file.
#[derive(Clone)]
pub struct DiskCache {
    root: Option<PathBuf>,
}
impl DiskCache {
    pub fn new(root: Option<PathBuf>) -> Self {
        Self { root }
    }
    pub fn path(&self, relative: &str) -> Option<PathBuf> {
        self.root.as_ref().map(|r| r.join(relative))
    }
    pub fn read(&self, relative: &str) -> Option<Vec<u8>> {
        std::fs::read(self.path(relative)?).ok()
    }
    pub fn exists(&self, relative: &str) -> bool {
        self.path(relative).is_some_and(|p| p.is_file())
    }
    pub fn write(&self, relative: &str, bytes: &[u8]) {
        let Some(path) = self.path(relative) else {
            return;
        };
        if let Err(e) = write_atomic(&path, bytes) {
            log::warn!("Earth cache: could not write {}: {e}", path.display());
        }
    }
    pub fn remove(&self, relative: &str) {
        if let Some(path) = self.path(relative) {
            let _ = std::fs::remove_file(path);
        }
    }
}
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let temp = dir.join(format!(
        "part{}-{}.part",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&temp, bytes)?;
    std::fs::rename(&temp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&temp);
    })
}

type Slot<V> = Arc<(Mutex<Option<Result<Arc<V>, Missing>>>, Condvar)>;

/// Decoded pieces by key: kept least recently used first out, loaded once however many ask.
pub struct Flight<K, V> {
    state: Mutex<FlightState<K, V>>,
    capacity: usize,
}
struct FlightState<K, V> {
    done: HashMap<K, (Arc<V>, u64)>,
    loading: HashMap<K, Slot<V>>,
    clock: u64,
}
impl<K: Hash + Eq + Clone, V> Flight<K, V> {
    pub fn new(capacity: usize) -> Self {
        Self {
            state: Mutex::new(FlightState {
                done: HashMap::new(),
                loading: HashMap::new(),
                clock: 0,
            }),
            capacity: capacity.max(1),
        }
    }
    pub fn peek(&self, key: &K) -> Option<Arc<V>> {
        let mut state = self.state.lock().unwrap();
        state.clock += 1;
        let clock = state.clock;
        state.done.get_mut(key).map(|(v, used)| {
            *used = clock;
            v.clone()
        })
    }
    /// The piece, loading it with `load` unless it is kept or another thread is loading it.
    /// Failures are not kept: the next caller tries again.
    pub fn get(
        &self,
        key: &K,
        load: impl FnOnce() -> Result<V, Missing>,
    ) -> Result<Arc<V>, Missing> {
        let slot = {
            let mut state = self.state.lock().unwrap();
            state.clock += 1;
            let clock = state.clock;
            if let Some((v, used)) = state.done.get_mut(key) {
                *used = clock;
                return Ok(v.clone());
            }
            if let Some(slot) = state.loading.get(key) {
                Err(slot.clone())
            } else {
                let slot: Slot<V> = Arc::new((Mutex::new(None), Condvar::new()));
                state.loading.insert(key.clone(), slot.clone());
                Ok(slot)
            }
        };
        match slot {
            Err(theirs) => {
                let mut result = theirs.0.lock().unwrap();
                while result.is_none() {
                    result = theirs.1.wait(result).unwrap();
                }
                result.clone().unwrap()
            }
            Ok(mine) => {
                // A panicking loader must not leave waiters hanging.
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(load))
                    .unwrap_or_else(|_| Err(Missing::Failed("loader panicked".into())))
                    .map(Arc::new);
                {
                    let mut state = self.state.lock().unwrap();
                    state.loading.remove(key);
                    if let Ok(v) = &result {
                        let clock = state.clock;
                        state.done.insert(key.clone(), (v.clone(), clock));
                        if state.done.len() > self.capacity + self.capacity / 4 {
                            let mut ages: Vec<u64> =
                                state.done.values().map(|(_, used)| *used).collect();
                            let cut = ages.len() - self.capacity;
                            let (_, &mut oldest, _) = ages.select_nth_unstable(cut);
                            state.done.retain(|_, (_, used)| *used > oldest);
                        }
                    }
                }
                *mine.0.lock().unwrap() = Some(result.clone());
                mine.1.notify_all();
                result
            }
        }
    }
    pub fn len(&self) -> usize {
        self.state.lock().unwrap().done.len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Runs every job on up to `threads` threads and waits for all of them.
pub fn run_all(jobs: Vec<Box<dyn FnOnce() + Send + '_>>, threads: usize) {
    let queue = Mutex::new(jobs);
    std::thread::scope(|scope| {
        for _ in 0..threads.max(1) {
            scope.spawn(|| {
                loop {
                    let job = queue.lock().unwrap().pop();
                    match job {
                        Some(job) => job(),
                        None => break,
                    }
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_piece_loads_once_for_every_caller() {
        let flight: Flight<u32, u32> = Flight::new(8);
        let loads = AtomicU64::new(0);
        std::thread::scope(|s| {
            for _ in 0..8 {
                s.spawn(|| {
                    let v = flight
                        .get(&7, || {
                            loads.fetch_add(1, Ordering::Relaxed);
                            std::thread::sleep(Duration::from_millis(20));
                            Ok(49)
                        })
                        .unwrap();
                    assert_eq!(*v, 49);
                });
            }
        });
        assert_eq!(loads.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn failures_are_tried_again_and_old_pieces_go_first() {
        let flight: Flight<u32, u32> = Flight::new(4);
        assert!(flight.get(&1, || Err(Missing::Absent)).is_err());
        assert_eq!(*flight.get(&1, || Ok(1)).unwrap(), 1);
        for k in 2..=6 {
            flight.get(&k, || Ok(k)).unwrap();
            flight.peek(&1);
        }
        assert!(flight.len() <= 5);
        assert!(
            flight.peek(&1).is_some(),
            "the piece used most recently stays"
        );
    }
}
