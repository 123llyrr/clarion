//! Two ownership transitions that do not require a GL context.
//!
//! Allocation is lazy for an existing bank. An override is first prepared in
//! isolation, then committed in one slot exchange; only the displaced owner is
//! returned for context-aware retirement.
use std::cell::RefCell;

pub(super) fn build_missing<T>(present: bool, build: impl FnOnce() -> T) -> Option<T> {
    if present {
        None
    } else {
        Some(build())
    }
}

pub(super) struct Replacement<T>(Option<T>);

impl<T> Replacement<T> {
    pub(super) fn prepare<E>(
        source: Option<&str>,
        compile: impl FnOnce(&str) -> Result<T, E>,
    ) -> Result<Self, E> {
        source.map(compile).transpose().map(Self)
    }

    pub(super) fn commit(self, slot: &RefCell<Option<T>>) -> Option<T> {
        slot.replace(self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    #[test]
    fn repeated_initialization_allocates_and_retires_nothing() {
        struct Allocation(Rc<Cell<usize>>);
        impl Drop for Allocation {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }
        let creates = Cell::new(0);
        let retires = Rc::new(Cell::new(0));
        let mut bank = None;
        for _ in 0..100 {
            if let Some(value) = build_missing(bank.is_some(), || {
                creates.set(creates.get() + 1);
                Allocation(retires.clone())
            }) {
                bank = Some(value);
            }
        }
        assert_eq!(creates.get(), 1);
        assert_eq!(retires.get(), 0);
        drop(bank);
        assert_eq!(retires.get(), 1);
    }

    #[test]
    fn failed_preparation_keeps_the_selected_owner_and_live_draw_reference() {
        let owner = Rc::new("last valid program");
        let draw = owner.clone();
        let slot = RefCell::new(Some(owner.clone()));
        let result = Replacement::<Rc<&str>>::prepare(Some("invalid"), |_| Err("compile failed"));
        assert_eq!(result.err(), Some("compile failed"));
        assert!(Rc::ptr_eq(slot.borrow().as_ref().unwrap(), &draw));
        assert_eq!(Rc::strong_count(&owner), 3);
    }

    #[test]
    fn prepared_program_is_invisible_until_commit_and_retirement_keeps_draw_alive() {
        let old = Rc::new("old");
        let draw = old.clone();
        let slot = RefCell::new(Some(old.clone()));
        let candidate = Rc::new("new");
        let prepared =
            Replacement::prepare(Some("new"), |_| Ok::<_, ()>(candidate.clone())).unwrap();
        assert!(Rc::ptr_eq(slot.borrow().as_ref().unwrap(), &old));
        let retired = prepared.commit(&slot).unwrap();
        assert!(Rc::ptr_eq(&retired, &old));
        assert!(Rc::ptr_eq(slot.borrow().as_ref().unwrap(), &candidate));
        drop(retired);
        assert_eq!(*draw, "old");
        assert_eq!(Rc::strong_count(&old), 2);
    }

    #[test]
    fn reset_returns_previous_owner_without_invoking_the_compiler() {
        let slot = RefCell::new(Some(17));
        let reset = Replacement::<i32>::prepare(None, |_| -> Result<i32, ()> {
            panic!("reset must not allocate a program")
        })
        .unwrap();
        assert_eq!(reset.commit(&slot), Some(17));
        assert_eq!(*slot.borrow(), None);
    }
}
