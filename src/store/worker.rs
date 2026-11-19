//! The library's connection lives on its own thread. The interface sends
//! closures and awaits their results, so SQLite never runs on the UI thread.
//! Jobs run in the order sent: a query issued after a like sees the like.
use super::Store;
use anyhow::{Result, anyhow};
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

/// A superseded query, or a scan stopped before it finished.
#[derive(Debug)]
pub struct Cancelled;
impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Cancelled")
    }
}
impl std::error::Error for Cancelled {}
pub fn is_cancelled(e: &anyhow::Error) -> bool {
    e.is::<Cancelled>()
        || e.downcast_ref::<rusqlite::Error>().is_some_and(|e| {
            e.sqlite_error_code() == Some(rusqlite::ErrorCode::OperationInterrupted)
        })
}

/// Issues generations for one view. A query whose generation is no longer
/// current is skipped, or interrupted if it is already running.
#[derive(Clone, Default)]
pub struct Ticket(Arc<AtomicU64>);
impl Ticket {
    pub fn next(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }
    pub fn is_current(&self, generation: u64) -> bool {
        self.0.load(Ordering::SeqCst) == generation
    }
}

type Job = Box<dyn FnOnce(Result<&mut Store, &str>) + Send>;

const MAX_PENDING: usize = 128;
type Request = (Option<Ticket>, Job);

#[derive(Default)]
struct Mailbox {
    state: Mutex<QueueState>,
    ready: Condvar,
}

#[derive(Default)]
struct QueueState {
    jobs: VecDeque<Request>,
    closed: bool,
}

struct Client {
    mailbox: Arc<Mailbox>,
    path: PathBuf,
}

impl Drop for Client {
    fn drop(&mut self) {
        self.mailbox.state.lock().unwrap().closed = true;
        self.mailbox.ready.notify_one();
    }
}

#[derive(Clone)]
pub struct Handle(Arc<Client>);

impl Handle {
    pub fn spawn(path: PathBuf) -> Self {
        let mailbox = Arc::new(Mailbox::default());
        let client = Self(Arc::new(Client {
            mailbox: mailbox.clone(),
            path: path.clone(),
        }));
        std::thread::Builder::new()
            .name("disqro-library".into())
            .spawn(move || {
                let mut store = Store::open(&path).map_err(|e| format!("{e:#}"));
                loop {
                    let mut state = mailbox.state.lock().unwrap();
                    while state.jobs.is_empty() && !state.closed {
                        state = mailbox.ready.wait(state).unwrap();
                    }
                    let Some((_, job)) = state.jobs.pop_front() else {
                        break;
                    };
                    drop(state);
                    match &mut store {
                        Ok(store) => job(Ok(store)),
                        Err(e) => job(Err(e)),
                    }
                }
            })
            .expect("start library worker");
        client
    }
    pub fn path(&self) -> &std::path::Path {
        &self.0.path
    }

    pub fn call<T, F>(&self, f: F) -> impl Future<Output = Result<T>> + use<T, F>
    where
        T: Send + 'static,
        F: FnOnce(&mut Store) -> Result<T> + Send + 'static,
    {
        self.submit(None, move |store| {
            f(store.map_err(|e| anyhow!("The library is unavailable: {e}"))?)
        })
    }

    /// FIFO barrier; a failed database open does not prevent workspace saving.
    pub fn drain(&self) -> impl Future<Output = Result<()>> + use<> {
        self.submit(None, |_| Ok(()))
    }

    fn submit<T, F>(
        &self,
        ticket: Option<Ticket>,
        f: F,
    ) -> impl Future<Output = Result<T>> + use<T, F>
    where
        T: Send + 'static,
        F: FnOnce(Result<&mut Store, &str>) -> Result<T> + Send + 'static,
    {
        let (tx, rx) = async_channel::bounded(1);
        let mailbox = &self.0.mailbox;
        let mut state = mailbox.state.lock().unwrap();
        if let Some(ticket) = &ticket {
            state
                .jobs
                .retain(|(older, _)| !older.as_ref().is_some_and(|t| Arc::ptr_eq(&t.0, &ticket.0)));
        }
        let full = state.jobs.len() >= MAX_PENDING;
        if !full {
            state.jobs.push_back((
                ticket,
                Box::new(move |store| {
                    let _ = tx.try_send(f(store));
                }),
            ));
            mailbox.ready.notify_one();
        }
        async move {
            anyhow::ensure!(!full, "The library is busy. Try the action again.");
            rx.recv().await.map_err(|_| Cancelled)?
        }
    }
    /// Runs a read that a newer generation of `ticket` supersedes.
    pub fn query<T, F>(
        &self,
        ticket: &Ticket,
        generation: u64,
        f: F,
    ) -> impl Future<Output = Result<T>> + use<T, F>
    where
        T: Send + 'static,
        F: FnOnce(&Store) -> Result<T> + Send + 'static,
    {
        let ticket = ticket.clone();
        self.submit(Some(ticket.clone()), move |store| {
            let store = store.map_err(|e| anyhow!("The library is unavailable: {e}"))?;
            if !ticket.is_current(generation) {
                return Err(Cancelled.into());
            }
            let watch = ticket.clone();
            store
                .db
                .progress_handler(4096, Some(move || !watch.is_current(generation)))?;
            let result = f(store);
            store.db.progress_handler(0, None::<fn() -> bool>)?;
            result
        })
    }
}
