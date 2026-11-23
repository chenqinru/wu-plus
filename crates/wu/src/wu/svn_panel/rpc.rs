use futures::{
    StreamExt,
    channel::{mpsc, oneshot},
};
use serde_json::{Value, json};
use smol::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use std::{
    collections::HashMap,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
use util::command::{Child, Stdio, new_command};

pub(super) const VERSION: u64 = 1;

#[derive(Clone, Debug)]
pub(super) struct Error {
    pub kind: String,
    pub message: String,
}

impl Error {
    fn transport(message: impl Into<String>) -> Self {
        Self {
            kind: "transport".into(),
            message: message.into(),
        }
    }
}

type Reply = Result<Value, Error>;
type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Reply>>>>;

pub(super) struct Client {
    child: Option<Child>,
    outgoing: Option<mpsc::UnboundedSender<String>>,
    pending: Pending,
    next: AtomicU64,
    worktree_diff: AtomicBool,
    focused_worktree_diff: AtomicBool,
    commit_diff: AtomicBool,
    revision_review: AtomicBool,
    focused_revision_review: AtomicBool,
    switch_targets: AtomicBool,
}

impl Client {
    pub(super) async fn connect(
        extension_dir: &Path,
        executable: &Path,
    ) -> Result<Arc<Self>, Error> {
        let client = Self::start(extension_dir, executable)?;
        let result = client
            .request(client.next_id(), "initialize", json!({}))
            .await?;
        if result["protocol_version"].as_u64() != Some(VERSION) {
            return Err(Error {
                kind: "protocol".into(),
                message: "The SVN plugin protocol is incompatible.".into(),
            });
        }
        client.set_worktree_diff(result["worktree_diff"].as_bool().unwrap_or(false));
        client.focused_worktree_diff.store(
            result["focused_worktree_diff"].as_bool().unwrap_or(false),
            Ordering::Relaxed,
        );
        client.set_revision_review(result["revision_review"].as_bool().unwrap_or(false));
        client.focused_revision_review.store(
            result["focused_revision_review"].as_bool().unwrap_or(false),
            Ordering::Relaxed,
        );
        client.set_commit_diff(result["commit_diff"].as_bool().unwrap_or(false));
        client.switch_targets.store(
            result["switch_targets"].as_bool().unwrap_or(false),
            Ordering::Relaxed,
        );
        Ok(client)
    }

    pub fn start(extension_dir: &Path, executable: &Path) -> Result<Arc<Self>, Error> {
        use std::path::Component;
        if executable.as_os_str().is_empty()
            || executable
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(Error::transport(
                "The provider executable must be inside the installed extension.",
            ));
        }
        let canonical = |path: &Path| {
            path.canonicalize()
                .map_err(|error| Error::transport(error.to_string()))
        };
        let extension_dir = canonical(extension_dir)?;
        let executable = canonical(&extension_dir.join(executable))?;
        if !executable.starts_with(&extension_dir) {
            return Err(Error::transport(
                "The provider executable points outside the installed extension.",
            ));
        }
        let mut child = new_command(executable)
            // A development extension may live in a macOS-protected folder.
            // Sparkle relaunches the IDE without any transient folder access.
            .current_dir(paths::data_dir())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| Error::transport(error.to_string()))?;
        let mut stdin = child.stdin.take().expect("configured stdin");
        let stdout = child.stdout.take().expect("configured stdout");
        let pending = Pending::default();
        let (outgoing, mut outgoing_rx) = mpsc::unbounded::<String>();
        let writer_pending = pending.clone();
        smol::spawn(async move {
            while let Some(message) = outgoing_rx.next().await {
                if stdin.write_all(message.as_bytes()).await.is_err() {
                    abort_pending(&writer_pending);
                    break;
                }
            }
        })
        .detach();
        let reader_pending = pending.clone();
        smol::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Some(Ok(line)) = lines.next().await {
                let Ok(response) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                let Some(id) = response["id"].as_u64() else {
                    continue;
                };
                if let Ok(mut pending) = reader_pending.lock()
                    && let Some(send) = pending.remove(&id)
                {
                    let result = if let Some(error) = response.get("error") {
                        Err(Error {
                            kind: error["data"]["kind"].as_str().unwrap_or("provider").into(),
                            message: error["message"]
                                .as_str()
                                .unwrap_or("The SVN provider failed.")
                                .into(),
                        })
                    } else {
                        response
                            .get("result")
                            .cloned()
                            .ok_or_else(|| Error::transport("Invalid provider reply."))
                    };
                    let _ = send.send(result);
                }
            }
            abort_pending(&reader_pending);
        })
        .detach();
        Ok(Arc::new(Self {
            child: Some(child),
            outgoing: Some(outgoing),
            pending,
            next: AtomicU64::new(1),
            worktree_diff: AtomicBool::new(false),
            focused_worktree_diff: AtomicBool::new(false),
            commit_diff: AtomicBool::new(false),
            revision_review: AtomicBool::new(false),
            focused_revision_review: AtomicBool::new(false),
            switch_targets: AtomicBool::new(false),
        }))
    }

    pub(super) fn set_worktree_diff(&self, available: bool) {
        self.worktree_diff.store(available, Ordering::Relaxed);
    }

    pub(super) fn has_worktree_diff(&self) -> bool {
        self.worktree_diff.load(Ordering::Relaxed)
    }

    pub(super) fn has_focused_worktree_diff(&self) -> bool {
        self.focused_worktree_diff.load(Ordering::Relaxed)
    }

    pub(super) fn set_commit_diff(&self, available: bool) {
        self.commit_diff.store(available, Ordering::Relaxed);
    }

    pub(super) fn has_commit_diff(&self) -> bool {
        self.commit_diff.load(Ordering::Relaxed)
    }

    pub(super) fn set_revision_review(&self, available: bool) {
        self.revision_review.store(available, Ordering::Relaxed);
    }

    pub(super) fn has_revision_review(&self) -> bool {
        self.revision_review.load(Ordering::Relaxed)
    }

    pub(super) fn has_focused_revision_review(&self) -> bool {
        self.focused_revision_review.load(Ordering::Relaxed)
    }

    pub(super) fn has_switch_targets(&self) -> bool {
        self.switch_targets.load(Ordering::Relaxed)
    }

    pub fn next_id(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed)
    }

    fn send(&self, request: Value) -> Result<(), Error> {
        self.outgoing
            .as_ref()
            .expect("active client")
            .unbounded_send(format!("{request}\n"))
            .map_err(|_| Error::transport("The SVN provider connection was closed."))
    }

    pub async fn request(&self, id: u64, method: &str, params: Value) -> Reply {
        let (send, receive) = oneshot::channel();
        self.pending
            .lock()
            .map_err(|_| Error::transport("The SVN provider connection was closed."))?
            .insert(id, send);
        if let Err(error) =
            self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
        {
            if let Ok(mut pending) = self.pending.lock() {
                pending.remove(&id);
            }
            return Err(error);
        }
        receive
            .await
            .map_err(|_| Error::transport("The SVN provider did not respond."))?
    }

    pub fn cancel(&self, id: u64) {
        let _ = self.send(json!({"jsonrpc":"2.0","method":"$/cancelRequest","params":{"id":id}}));
    }
}

fn abort_pending(pending: &Pending) {
    if let Ok(mut pending) = pending.lock() {
        for (_, sender) in pending.drain() {
            let _ = sender.send(Err(Error::transport("The SVN provider stopped.")));
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // Closing the writer asks the provider to cancel its SVN subprocesses.
        self.outgoing.take();
        if let Some(mut child) = self.child.take() {
            smol::spawn(async move {
                let _ = child.status().await;
            })
            .detach();
        }
    }
}
