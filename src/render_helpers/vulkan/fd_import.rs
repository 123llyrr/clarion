//! Import failure ownership varies in deployed drivers. A fixed pre-import budget
//! bounds uncertain descriptors without ever closing a number the driver may
//! already have consumed and another thread may have reused.
use std::os::fd::{IntoRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicUsize, Ordering};

pub(super) const MAX_UNCERTAIN_IMPORTS: usize = 64;
static FAILURE_BUDGET: FailureBudget = FailureBudget::new(MAX_UNCERTAIN_IMPORTS);

pub(super) enum ImportError<E> {
    Exhausted,
    Operation(E),
}

struct FailureBudget {
    occupied: AtomicUsize,
    limit: usize,
}
impl FailureBudget {
    const fn new(limit: usize) -> Self {
        Self {
            occupied: AtomicUsize::new(0),
            limit,
        }
    }
    fn reserve(&self) -> Option<Permit<'_>> {
        self.occupied
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |occupied| {
                (occupied < self.limit).then(|| occupied + 1)
            })
            .ok()
            .map(|_| Permit {
                budget: self,
                release: true,
            })
    }
}
struct Permit<'a> {
    budget: &'a FailureBudget,
    release: bool,
}
impl Permit<'_> {
    fn quarantine(mut self) {
        self.release = false;
    }
}
impl Drop for Permit<'_> {
    fn drop(&mut self) {
        if self.release {
            self.budget.occupied.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

struct Handoff<'a> {
    raw: RawFd,
    permit: Option<Permit<'a>>,
    definitely_closed: fn(RawFd) -> bool,
}
impl Drop for Handoff<'_> {
    fn drop(&mut self) {
        if let Some(permit) = self.permit.take() {
            // EBADF proves there is no descriptor to retain at this instant.
            // Every other outcome is ambiguous: never check identity then close,
            // since another thread can reuse the number between those calls.
            if !(self.definitely_closed)(self.raw) {
                permit.quarantine();
            }
        }
    }
}

fn definitely_closed(raw: RawFd) -> bool {
    (unsafe { libc::fcntl(raw, libc::F_GETFD) }) == -1
        && std::io::Error::last_os_error().raw_os_error() == Some(libc::EBADF)
}

pub(super) fn import<T, E>(
    duplicate: impl FnOnce() -> Result<OwnedFd, E>,
    operation: impl FnOnce(RawFd) -> Result<T, E>,
) -> Result<T, ImportError<E>> {
    with_budget(&FAILURE_BUDGET, duplicate, operation)
}
fn with_budget<T, E>(
    budget: &FailureBudget,
    duplicate: impl FnOnce() -> Result<OwnedFd, E>,
    operation: impl FnOnce(RawFd) -> Result<T, E>,
) -> Result<T, ImportError<E>> {
    with_budget_and_probe(budget, duplicate, operation, definitely_closed)
}
fn with_budget_and_probe<T, E>(
    budget: &FailureBudget,
    duplicate: impl FnOnce() -> Result<OwnedFd, E>,
    operation: impl FnOnce(RawFd) -> Result<T, E>,
    definitely_closed: fn(RawFd) -> bool,
) -> Result<T, ImportError<E>> {
    let permit = budget.reserve().ok_or(ImportError::Exhausted)?;
    let raw = duplicate().map_err(ImportError::Operation)?.into_raw_fd();
    let mut handoff = Handoff {
        raw,
        permit: Some(permit),
        definitely_closed,
    };
    match operation(raw) {
        Ok(value) => {
            // A successful Vulkan import transfers the descriptor to the driver.
            // Its ownership is no longer uncertain and consumes no failure slot.
            drop(handoff.permit.take());
            Ok(value)
        }
        Err(error) => Err(ImportError::Operation(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::os::fd::{AsRawFd, FromRawFd};

    fn descriptor() -> OwnedFd {
        std::fs::File::open("/dev/null").unwrap().into()
    }
    fn close_known_fixture_descriptor(raw: RawFd) {
        // The fixture created and still owns this exact descriptor. Production
        // failure cleanup deliberately performs no analogous reconstruction.
        drop(unsafe { OwnedFd::from_raw_fd(raw) });
    }
    #[test]
    fn successful_and_consumed_failure_imports_release_admission_slots() {
        assert!(definitely_closed(-1));
        let live = descriptor();
        assert!(!definitely_closed(live.as_raw_fd()));
        drop(live);
        let budget = FailureBudget::new(1);
        let success: Result<(), ImportError<()>> = with_budget(
            &budget,
            || Ok(descriptor()),
            |raw| {
                close_known_fixture_descriptor(raw);
                Ok(())
            },
        );
        assert!(success.is_ok());
        assert_eq!(budget.occupied.load(Ordering::Acquire), 0);
        // Inject a definite EBADF observation rather than racing unrelated
        // parallel test file opens after this fixture closes its descriptor.
        let failure: Result<(), ImportError<()>> = with_budget_and_probe(
            &budget,
            || Ok(descriptor()),
            |raw| {
                close_known_fixture_descriptor(raw);
                Err(())
            },
            |_| true,
        );
        assert!(matches!(failure, Err(ImportError::Operation(()))));
        assert_eq!(budget.occupied.load(Ordering::Acquire), 0);
    }
    #[test]
    fn live_failed_imports_are_bounded_and_refused_before_duplicate_or_driver_call() {
        let budget = FailureBudget::new(2);
        let mut uncertain = Vec::new();
        for _ in 0..2 {
            let raw = Cell::new(-1);
            let result: Result<(), ImportError<()>> = with_budget(
                &budget,
                || Ok(descriptor()),
                |fd| {
                    raw.set(fd);
                    Err(())
                },
            );
            assert!(matches!(result, Err(ImportError::Operation(()))));
            assert!(unsafe { libc::fcntl(raw.get(), libc::F_GETFD) } >= 0);
            uncertain.push(raw.get());
        }
        let duplicate_called = Cell::new(false);
        let driver_called = Cell::new(false);
        let result: Result<(), ImportError<()>> = with_budget(
            &budget,
            || {
                duplicate_called.set(true);
                Ok(descriptor())
            },
            |_| {
                driver_called.set(true);
                Ok(())
            },
        );
        assert!(matches!(result, Err(ImportError::Exhausted)));
        assert!(!duplicate_called.get());
        assert!(!driver_called.get());
        assert_eq!(budget.occupied.load(Ordering::Acquire), 2);
        for raw in uncertain {
            close_known_fixture_descriptor(raw);
        }
        // Cleanup in the fixture cannot resurrect a permanently uncertain slot.
        assert_eq!(budget.occupied.load(Ordering::Acquire), 2);
    }
    #[test]
    fn reused_failed_import_descriptor_is_never_closed_by_cleanup() {
        let budget = FailureBudget::new(1);
        let replacement = std::fs::File::open("/dev/zero").unwrap();
        let reused = Cell::new(-1);
        let result: Result<(), ImportError<()>> = with_budget(
            &budget,
            || Ok(descriptor()),
            |raw| {
                // Atomically models a driver consuming the import followed by an
                // unrelated file occupying its number before the error returns.
                assert_eq!(unsafe { libc::dup2(replacement.as_raw_fd(), raw) }, raw);
                reused.set(raw);
                Err(())
            },
        );
        assert!(matches!(result, Err(ImportError::Operation(()))));
        assert!(unsafe { libc::fcntl(reused.get(), libc::F_GETFD) } >= 0);
        assert_eq!(budget.occupied.load(Ordering::Acquire), 1);
        let mut byte = 1u8;
        assert_eq!(
            unsafe { libc::read(reused.get(), (&mut byte as *mut u8).cast(), 1) },
            1
        );
        assert_eq!(byte, 0);
        close_known_fixture_descriptor(reused.get());
    }
    #[test]
    fn failed_duplication_releases_its_slot_without_calling_the_driver() {
        let budget = FailureBudget::new(1);
        let called = Cell::new(false);
        let result: Result<(), ImportError<()>> = with_budget(
            &budget,
            || Err(()),
            |_| {
                called.set(true);
                Ok(())
            },
        );
        assert!(matches!(result, Err(ImportError::Operation(()))));
        assert!(!called.get());
        assert_eq!(budget.occupied.load(Ordering::Acquire), 0);
    }
    #[test]
    fn panicking_driver_boundary_cannot_escape_the_failure_budget() {
        let budget = FailureBudget::new(1);
        let raw = Cell::new(-1);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _: Result<(), ImportError<()>> = with_budget(
                &budget,
                || Ok(descriptor()),
                |fd| {
                    raw.set(fd);
                    panic!("injected boundary panic");
                },
            );
        }));
        assert!(result.is_err());
        assert_eq!(budget.occupied.load(Ordering::Acquire), 1);
        close_known_fixture_descriptor(raw.get());
    }
}
