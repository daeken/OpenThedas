//! A download running on its own thread, and how the launcher talks to it.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use crate::{Login, Request, Secret, Store};

/// What a download tells the launcher.
#[derive(Clone, Debug)]
pub enum Event {
    /// What is happening now, in a few words.
    Status(String),
    /// Steam QR sign-in: show this URL as a QR code for the Steam mobile app to scan.
    SteamQr(String),
    /// Steam wants a Steam Guard code: from email if `email`, else from the mobile app.
    /// Answer with [`Reply::GuardCode`].
    NeedGuardCode { email: bool },
    /// The last Steam Guard code was wrong; ask again.
    WrongGuardCode,
    /// Steam waits for the player to approve the sign-in in the Steam mobile app.
    NeedMobileConfirm,
    /// Signed in. The credentials the player typed are dropped from here on.
    SignedIn,
    /// What will be downloaded: total bytes and files.
    Plan { bytes: u64, files: u64 },
    /// Bytes done so far, of `total`.
    Progress { done: u64, total: u64 },
    /// Finished: the game is in `root`.
    Done { root: PathBuf },
    /// Stopped with an error, worded for the player.
    Failed(String),
    /// Stopped because the player cancelled. Files already downloaded stay; starting again
    /// into the same folder picks up where it stopped.
    Cancelled,
}

/// What the launcher tells a download.
pub enum Reply {
    GuardCode(Secret),
}

/// A download in progress. Dropping it cancels the download.
pub struct Job {
    store: Store,
    events: mpsc::Receiver<Event>,
    replies: tokio::sync::mpsc::UnboundedSender<Reply>,
    cancel: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Job {
    /// Starts downloading `req` from the store `login` names, signing in with it.
    pub fn start(req: Request, login: Login) -> Job {
        let store = login.store();
        let (etx, events) = mpsc::channel();
        let (replies, rrx) = tokio::sync::mpsc::unbounded_channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let ctx = Ctx {
            events: etx,
            replies: rrx,
            cancel: cancel.clone(),
        };
        let thread = std::thread::Builder::new()
            .name("store-download".into())
            .spawn(move || run(req, login, ctx))
            .ok();
        Job {
            store,
            events,
            replies,
            cancel,
            thread,
        }
    }

    pub fn store(&self) -> Store {
        self.store
    }

    /// The events since the last call.
    pub fn poll(&self) -> Vec<Event> {
        let mut out: Vec<Event> = self.events.try_iter().collect();
        if self.thread.is_none() && out.is_empty() {
            out.push(Event::Failed("Could not start the download.".into()));
        }
        out
    }

    pub fn reply(&self, r: Reply) {
        let _ = self.replies.send(r);
    }

    /// Asks the download to stop. It answers with [`Event::Cancelled`].
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Whether the download's thread has ended (after its last event).
    pub fn finished(&self) -> bool {
        self.thread.as_ref().is_none_or(|t| t.is_finished())
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// The download's side of a [`Job`].
pub(crate) struct Ctx {
    events: mpsc::Sender<Event>,
    replies: tokio::sync::mpsc::UnboundedReceiver<Reply>,
    cancel: Arc<AtomicBool>,
}

impl Ctx {
    pub(crate) fn send(&self, e: Event) {
        let _ = self.events.send(e);
    }

    pub(crate) fn status(&self, s: impl Into<String>) {
        self.send(Event::Status(s.into()));
    }

    /// The next reply from the launcher; `None` when it went away.
    pub(crate) async fn reply(&mut self) -> Option<Reply> {
        self.replies.recv().await
    }

    pub(crate) fn cancel_flag(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }

    pub(crate) fn sender(&self) -> mpsc::Sender<Event> {
        self.events.clone()
    }
}

/// Resolves when `flag` is set.
pub(crate) async fn until_cancelled(flag: Arc<AtomicBool>) {
    while !flag.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

/// A failure, worded for the player.
pub(crate) type Result<T> = std::result::Result<T, String>;

fn run(req: Request, login: Login, mut ctx: Ctx) {
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .thread_name("store-io")
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            ctx.send(Event::Failed(format!("Could not start the download: {e}")));
            return;
        }
    };
    let flag = ctx.cancel_flag();
    let events = ctx.sender();
    let result = rt.block_on(async {
        let work = async {
            match login {
                Login::Gog { code } => crate::gog::run(&req, code, &mut ctx).await,
                login => crate::steam::run(&req, login, &mut ctx).await,
            }
        };
        tokio::select! {
            r = work => Some(r),
            _ = until_cancelled(flag.clone()) => None,
        }
    });
    let _ = events.send(match result {
        None => Event::Cancelled,
        Some(Ok(root)) => Event::Done { root },
        Some(Err(e)) if flag.load(Ordering::Relaxed) => {
            log::info!("download stopped after cancel: {e}");
            Event::Cancelled
        }
        Some(Err(e)) => Event::Failed(e),
    });
    // Background tasks (chunk fetches) are dropped with the runtime, without waiting long.
    rt.shutdown_timeout(Duration::from_secs(2));
}

/// Makes `dir` and checks it is a folder the download may fill.
pub(crate) fn prepare_dest(dir: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("Could not make the folder {}: {e}", dir.display()))?;
    if !dir.is_dir() {
        return Err(format!("{} is not a folder.", dir.display()));
    }
    Ok(())
}
