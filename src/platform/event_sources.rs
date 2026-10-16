//! Owner-scoped calloop sources with destruction outside the loop's source borrow.
//!
//! Executor futures can own Async objects, which themselves retain the loop. A
//! stopped loop cannot cancel those futures by dropping its handle alone.

use calloop::{Dispatcher, EventSource, LoopHandle};

pub(crate) struct ScopedSource<S: EventSource + 'static, Data: 'static> {
    dispatcher: Option<Dispatcher<'static, S, Data>>,
    unregister: Option<Box<dyn FnOnce()>>,
}

impl<S: EventSource + 'static, Data: 'static> ScopedSource<S, Data> {
    pub(crate) fn insert<F>(
        event_loop: &LoopHandle<'static, Data>,
        source: S,
        callback: F,
    ) -> calloop::Result<Self>
    where
        F: FnMut(S::Event, &mut S::Metadata, &mut Data) -> S::Ret + 'static,
    {
        let dispatcher = Dispatcher::new(source, callback);
        let token = event_loop.register_dispatcher(dispatcher.clone())?;
        // calloop exposes downgrade() but does not re-export its weak-handle
        // type. Erase only this startup/teardown action, retaining a weak owner.
        let event_loop = event_loop.downgrade();
        Ok(Self {
            dispatcher: Some(dispatcher),
            unregister: Some(Box::new(move || {
                if let Some(event_loop) = event_loop.upgrade() {
                    event_loop.remove(token);
                }
            })),
        })
    }

    /// Call after dispatch has returned. Keeping our Dispatcher alive during
    /// remove is essential: Executor::Drop cancels Async futures, whose Drop
    /// removes their own IO sources and would otherwise reborrow the source slab.
    pub(crate) fn remove(&mut self) {
        if let Some(unregister) = self.unregister.take() {
            unregister();
        }
        // No calloop source/poll borrow remains here. Cancellation may re-enter
        // the loop to unregister nested IO or other owner-scoped sources.
        drop(self.dispatcher.take());
    }

    /// Borrow only the owned source, never the loop's source slab. This lets a
    /// stopped Display deliver destruction callbacks while its State is alive.
    pub(crate) fn with_source_mut<T>(&mut self, f: impl FnOnce(&mut S) -> T) -> T {
        let mut source = self.dispatcher.as_ref().unwrap().as_source_mut();
        f(&mut source)
    }
}

impl<S: EventSource + 'static, Data: 'static> Drop for ScopedSource<S, Data> {
    fn drop(&mut self) {
        self.remove();
    }
}
