//! Test-only fault injection: one thread-local one-shot hook per race the
//! suites need to force.
//!
//! Production call sites carry a single `#[cfg(test)] fire(...)` line, so a
//! production build has neither the slots nor the calls. A hook is consumed by
//! its first fire, and an unfired one is dropped with the test thread, so a
//! race that never happened cannot leak into the next test.

#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
use std::cell::Cell;
use std::cell::RefCell;
use std::path::PathBuf;
use std::thread::LocalKey;

/// The boxed hook a slot holds; the alias keeps the type readable.
type Hook<A, R> = RefCell<Option<Box<dyn FnOnce(A) -> R>>>;

/// One thread-local hook, consumed by its first fire.
pub(crate) struct FaultSlot<A, R = ()> {
    hook: Hook<A, R>,
}

impl<A, R> FaultSlot<A, R> {
    pub(crate) const fn new() -> Self {
        Self {
            hook: RefCell::new(None),
        }
    }

    fn install(&self, hook: impl FnOnce(A) -> R + 'static) {
        let mut slot = self.hook.borrow_mut();
        assert!(slot.is_none(), "a test fault hook is already installed");
        *slot = Some(Box::new(hook));
    }

    fn take(&self) -> Option<Box<dyn FnOnce(A) -> R>> {
        self.hook.borrow_mut().take()
    }
}

/// Install `hook` into `slot`; fail when one is already pending.
pub(crate) fn install<A, R>(
    slot: &'static LocalKey<FaultSlot<A, R>>,
    hook: impl FnOnce(A) -> R + 'static,
) {
    slot.with(|slot| slot.install(hook));
}

/// Fire `slot` once with `argument`; no installed hook is a no-op.
pub(crate) fn fire<A>(slot: &'static LocalKey<FaultSlot<A>>, argument: A) {
    fire_with(slot, argument, ());
}

/// Fire `slot` once with `argument`, or return `fallback` when no hook was
/// installed.
pub(crate) fn fire_with<A, R>(
    slot: &'static LocalKey<FaultSlot<A, R>>,
    argument: A,
    fallback: R,
) -> R {
    slot.with(|slot| match slot.take() {
        Some(hook) => hook(argument),
        None => fallback,
    })
}

thread_local! {
    /// Between a read and the verification that follows it, so a test can
    /// rename a file over the path while the snapshot is live.
    pub(crate) static READ_RACE: FaultSlot<()> = const { FaultSlot::new() };
    /// After the second content read and before the path check that follows
    /// it, so a test can replace the path inside that window.
    pub(crate) static READ_REREAD: FaultSlot<()> = const { FaultSlot::new() };
    /// After the loaded source was matched against its stamp and identity.
    #[cfg(windows)]
    pub(crate) static RECOVERY_SOURCE_MATCH: FaultSlot<()> = const { FaultSlot::new() };
    /// After the authenticated Skill root was opened and validated.
    pub(crate) static SKILL_ROOT_VALIDATED: FaultSlot<()> = const { FaultSlot::new() };
    /// Before the first commit mutation.
    pub(crate) static SAVE_COMMIT: FaultSlot<()> = const { FaultSlot::new() };
    /// After the commit's replace completed.
    pub(crate) static SAVE_POST_COMMIT: FaultSlot<()> = const { FaultSlot::new() };
    /// Between the staged and recovery names just before the swap.
    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    pub(crate) static SKILL_BEFORE_EXCHANGE: FaultSlot<(PathBuf, PathBuf)> = const { FaultSlot::new() };
    /// Before the staged and recovery names are unlinked.
    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    pub(crate) static SKILL_BEFORE_CLEANUP: FaultSlot<PathBuf> = const { FaultSlot::new() };
    /// Once, to force the unsupported rename-exchange fallback.
    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    static SKILL_FORCE_UNSUPPORTED_EXCHANGE: Cell<bool> = const { Cell::new(false) };
    /// After the destination was reserved, before it is removed.
    #[cfg(windows)]
    pub(crate) static SAVE_RESERVATION: FaultSlot<PathBuf, std::io::Result<()>> = const { FaultSlot::new() };
    /// After the production recovery root handle was opened.
    #[cfg(windows)]
    pub(crate) static ROOT_ACQUISITION: FaultSlot<()> = const { FaultSlot::new() };
}

#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
pub(crate) fn force_skill_exchange_unsupported() {
    SKILL_FORCE_UNSUPPORTED_EXCHANGE.with(|flag| flag.set(true));
}

#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
pub(crate) fn skill_exchange_forced_unsupported() -> bool {
    SKILL_FORCE_UNSUPPORTED_EXCHANGE.with(|flag| flag.replace(false))
}
