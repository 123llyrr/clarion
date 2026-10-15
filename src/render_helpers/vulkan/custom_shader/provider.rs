//! One fixed pure compiler worker. Capacity includes results awaiting owner delivery.
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};

use calloop::channel::Channel;

const CAPACITY: usize = 2;
struct Permit(Arc<AtomicUsize>);
impl Drop for Permit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Release);
    }
}
struct Request<P> {
    renderer: u64,
    kind: u8,
    generation: u64,
    source: String,
    cancelled: Arc<AtomicBool>,
    permit: Permit,
    reply: calloop::channel::SyncSender<CompileCompletion<P>>,
}
pub(crate) struct CompileCompletion<P> {
    pub renderer: u64,
    pub kind: u8,
    pub generation: u64,
    pub source: String,
    pub result: Result<P, String>,
    _permit: Permit,
}
pub(crate) struct CompileData<P> {
    pub renderer: u64,
    pub kind: u8,
    pub generation: u64,
    pub source: String,
    pub result: Result<P, String>,
}
impl<P> CompileCompletion<P> {
    /// Owner delivery ends the pure-work slot. GPU construction is a separate owner phase.
    pub fn into_data(self) -> CompileData<P> {
        let Self {
            renderer,
            kind,
            generation,
            source,
            result,
            _permit,
        } = self;
        drop(_permit);
        CompileData {
            renderer,
            kind,
            generation,
            source,
            result,
        }
    }
}
pub(super) struct Compiler<P> {
    requests: mpsc::SyncSender<Request<P>>,
    slots: Arc<AtomicUsize>,
}
pub(crate) struct Endpoint<P> {
    requests: mpsc::SyncSender<Request<P>>,
    slots: Arc<AtomicUsize>,
    reply: calloop::channel::SyncSender<CompileCompletion<P>>,
}
impl<P: Send + 'static> Compiler<P> {
    pub fn new(prepare: fn(&str, &AtomicBool) -> Result<P, String>) -> std::io::Result<Self> {
        let (requests, rx) = mpsc::sync_channel::<Request<P>>(CAPACITY);
        let slots = Arc::new(AtomicUsize::new(0));
        std::thread::Builder::new()
            .name("vulkan-shader-compiler".into())
            .spawn(move || {
                while let Ok(request) = rx.recv() {
                    let result = if request.cancelled.load(Ordering::Acquire) {
                        Err("shader compile cancelled".to_owned())
                    } else {
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            prepare(&request.source, &request.cancelled)
                        }))
                        .unwrap_or_else(|_| Err("shader compiler worker panicked".to_owned()))
                    };
                    // A retired renderer's closed receiver never terminates the shared worker.
                    let _ = request.reply.send(CompileCompletion {
                        renderer: request.renderer,
                        kind: request.kind,
                        generation: request.generation,
                        source: request.source,
                        result,
                        _permit: request.permit,
                    });
                }
            })?;
        Ok(Self { requests, slots })
    }
    pub fn endpoint(&self) -> (Endpoint<P>, Channel<CompileCompletion<P>>) {
        let (reply, completions) = calloop::channel::sync_channel(CAPACITY);
        (
            Endpoint {
                requests: self.requests.clone(),
                slots: self.slots.clone(),
                reply,
            },
            completions,
        )
    }
}
impl<P: Send + 'static> Endpoint<P> {
    pub fn request(
        &self,
        renderer: u64,
        kind: u8,
        generation: u64,
        source: String,
        cancelled: Arc<AtomicBool>,
    ) -> Result<(), String> {
        self.slots
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |slots| {
                (slots < CAPACITY).then_some(slots + 1)
            })
            .map_err(|_| "shader compiler capacity reached".to_owned())?;
        let permit = Permit(self.slots.clone());
        self.requests
            .try_send(Request {
                renderer,
                kind,
                generation,
                source,
                cancelled,
                permit,
                reply: self.reply.clone(),
            })
            .map_err(|_| "shader compiler unavailable".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn prepare(source: &str, cancel: &AtomicBool) -> Result<Vec<u8>, String> {
        if cancel.load(Ordering::Acquire) {
            return Err("cancelled".into());
        }
        match source {
            "bad" => Err("malformed GLSL".into()),
            "panic" => panic!("parser failure"),
            _ => Ok(source.as_bytes().to_vec()),
        }
    }
    #[test]
    fn capacity_covers_owner_undelivered_results_and_tags_are_preserved() {
        let service = Compiler::new(prepare).unwrap();
        let (compiler, results) = service.endpoint();
        let cancelled = Arc::new(AtomicBool::new(false));
        compiler
            .request(17, 1, 4, "first".into(), cancelled.clone())
            .unwrap();
        compiler
            .request(17, 2, 9, "second".into(), cancelled.clone())
            .unwrap();
        assert!(compiler
            .request(18, 1, 10, "third".into(), cancelled.clone())
            .is_err());
        let first = results.recv().unwrap();
        let second = results.recv().unwrap();
        assert_eq!(
            (
                first.renderer,
                first.kind,
                first.generation,
                first.source.as_str()
            ),
            (17, 1, 4, "first")
        );
        assert_eq!(first.result.as_ref().unwrap(), b"first");
        assert!(compiler
            .request(18, 1, 10, "third".into(), cancelled.clone())
            .is_err());
        let first = first.into_data();
        assert_eq!(first.result.unwrap(), b"first");
        // Owner conversion releases capacity before GPU preparation; retained pure data
        // contains no permit, context or event-loop state.
        compiler
            .request(18, 1, 10, "third".into(), cancelled.clone())
            .unwrap();
        drop(second);
        assert_eq!(results.recv().unwrap().result.unwrap(), b"third");
    }
    #[test]
    fn cancellation_and_failure_dont_poison_fixed_worker() {
        let service = Compiler::new(prepare).unwrap();
        let (compiler, results) = service.endpoint();
        compiler
            .request(1, 0, 1, "unused".into(), Arc::new(AtomicBool::new(true)))
            .unwrap();
        assert!(results.recv().unwrap().result.is_err());
        for (generation, source) in [(2, "bad"), (3, "panic"), (4, "recovered")] {
            compiler
                .request(
                    1,
                    0,
                    generation,
                    source.into(),
                    Arc::new(AtomicBool::new(false)),
                )
                .unwrap();
            let result = results.recv().unwrap();
            if source == "recovered" {
                assert_eq!(result.result.unwrap(), b"recovered");
            } else {
                assert!(result.result.is_err());
            }
        }
    }
    #[test]
    fn retired_renderer_receiver_does_not_spawn_or_poison_a_replacement_worker() {
        let service = Compiler::new(prepare).unwrap();
        let (old, results) = service.endpoint();
        old.request(1, 0, 1, "old".into(), Arc::new(AtomicBool::new(false)))
            .unwrap();
        drop(results);
        drop(old);
        let (new, results) = service.endpoint();
        new.request(2, 0, 1, "new".into(), Arc::new(AtomicBool::new(false)))
            .unwrap();
        assert_eq!(results.recv().unwrap().result.unwrap(), b"new");
    }
}
