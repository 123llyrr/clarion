//! Instance-local ownership for retained scene data.
//!
//! Adapters carry a non-cloneable registration, never a strong GPU owner. A
//! short-lived lease is allowed while preparing one frame. Resource destruction
//! runs outside the registry borrow so nested handle retirement cannot reenter it.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ops::Deref;
use std::rc::{Rc, Weak};

#[derive(Debug)]
struct RegistryInner<T> {
    next: Cell<u64>,
    entries: RefCell<HashMap<u64, Rc<T>>>,
}

#[derive(Debug)]
pub(crate) struct ResourceRegistry<T> {
    inner: Rc<RegistryInner<T>>,
}

impl<T> ResourceRegistry<T> {
    pub(crate) fn new() -> Self {
        Self {
            inner: Rc::new(RegistryInner {
                next: Cell::new(0),
                entries: RefCell::new(HashMap::new()),
            }),
        }
    }

    pub(crate) fn allocator(&self) -> ResourceAllocator<T> {
        ResourceAllocator {
            owner: Rc::downgrade(&self.inner),
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.inner.entries.borrow().len()
    }
}

#[derive(Debug)]
pub(crate) struct ResourceAllocator<T> {
    owner: Weak<RegistryInner<T>>,
}

impl<T> Clone for ResourceAllocator<T> {
    fn clone(&self) -> Self {
        Self {
            owner: self.owner.clone(),
        }
    }
}

impl<T> ResourceAllocator<T> {
    pub(crate) fn register(&self, value: T) -> ResourceHandle<T> {
        let owner = self.owner.upgrade().expect("scene owner is alive");
        let key = owner
            .next
            .get()
            .checked_add(1)
            .expect("scene keys exhausted");
        owner.next.set(key);
        let previous = owner.entries.borrow_mut().insert(key, Rc::new(value));
        debug_assert!(previous.is_none());
        ResourceHandle {
            key,
            owner: self.owner.clone(),
        }
    }
}

/// Registration identity survives moves; its key is never reused in an instance.
#[derive(Debug)]
pub(crate) struct ResourceHandle<T> {
    key: u64,
    owner: Weak<RegistryInner<T>>,
}

impl<T> ResourceHandle<T> {
    pub(crate) fn lease(&self) -> ResourceLease<T> {
        let owner = self.owner.upgrade().expect("scene owner is alive");
        let value = owner
            .entries
            .borrow()
            .get(&self.key)
            .cloned()
            .expect("scene registration is live");
        ResourceLease(value)
    }
}

impl<T> Drop for ResourceHandle<T> {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.upgrade() {
            let value = owner.entries.borrow_mut().remove(&self.key);
            drop(value);
        }
    }
}

#[derive(Debug)]
pub(crate) struct ResourceLease<T>(Rc<T>);

impl<T> Deref for ResourceLease<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Probe(Rc<Cell<usize>>);
    impl Drop for Probe {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    #[test]
    fn registrations_are_weak_and_frame_leases_retire_after_unregister() {
        let drops = Rc::new(Cell::new(0));
        let registry = ResourceRegistry::new();
        let allocator = registry.allocator();
        let handle = allocator.register(Probe(drops.clone()));
        let key = handle.key;
        let lease = handle.lease();
        drop(handle);
        assert_eq!(registry.len(), 0);
        assert_eq!(drops.get(), 0);
        let next = allocator.register(Probe(drops.clone()));
        assert_ne!(key, next.key);
        drop(registry);
        assert!(allocator.owner.upgrade().is_none());
        assert_eq!(drops.get(), 1);
        drop(next);
        drop(lease);
        assert_eq!(drops.get(), 2);
    }

    #[test]
    fn resource_drop_can_unregister_a_second_resource_without_borrowing_registry() {
        struct Nested(Option<ResourceHandle<Nested>>);
        impl Drop for Nested {
            fn drop(&mut self) {
                drop(self.0.take());
            }
        }
        let registry = ResourceRegistry::new();
        let allocator = registry.allocator();
        let child = allocator.register(Nested(None));
        let parent = allocator.register(Nested(Some(child)));
        drop(parent);
        assert_eq!(registry.len(), 0);
    }
}
