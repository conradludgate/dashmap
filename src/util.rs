//! This module is full of hackery and dark magic.
//! Either spend a day fixing it and quietly submit a PR or don't mention it to anybody.
use core::cell::UnsafeCell;
use core::{mem, ptr};
use std::marker::PhantomData;
use std::mem::ManuallyDrop;

use lock_api::{RawRwLock, RawRwLockDowngrade, RwLockReadGuard, RwLockWriteGuard};

pub const fn ptr_size_bits() -> usize {
    mem::size_of::<usize>() * 8
}

pub fn map_in_place_2<T, U, F: FnOnce(U, T) -> T>((k, v): (U, &mut T), f: F) {
    unsafe {
        // # Safety
        //
        // If the closure panics, we must abort otherwise we could double drop `T`
        let promote_panic_to_abort = AbortOnPanic;

        ptr::write(v, f(k, ptr::read(v)));

        // If we made it here, the calling thread could have already have panicked, in which case
        // We know that the closure did not panic, so don't bother checking.
        std::mem::forget(promote_panic_to_abort);
    }
}

/// A simple wrapper around `T`
///
/// This is to prevent UB when using `HashMap::get_key_value`, because
/// `HashMap` doesn't expose an api to get the key and value, where
/// the value is a `&mut T`.
///
/// See [#10](https://github.com/xacrimon/dashmap/issues/10) for details
///
/// This type is meant to be an implementation detail, but must be exposed due to the `Dashmap::shards`
#[repr(transparent)]
pub struct SharedValue<T> {
    value: UnsafeCell<T>,
}

impl<T: Clone> Clone for SharedValue<T> {
    fn clone(&self) -> Self {
        let inner = self.get().clone();

        Self {
            value: UnsafeCell::new(inner),
        }
    }
}

unsafe impl<T: Send> Send for SharedValue<T> {}

unsafe impl<T: Sync> Sync for SharedValue<T> {}

impl<T> SharedValue<T> {
    /// Create a new `SharedValue<T>`
    pub const fn new(value: T) -> Self {
        Self {
            value: UnsafeCell::new(value),
        }
    }

    /// Get a shared reference to `T`
    pub fn get(&self) -> &T {
        unsafe { &*self.value.get() }
    }

    /// Get an unique reference to `T`
    pub fn get_mut(&mut self) -> &mut T {
        unsafe { &mut *self.value.get() }
    }

    /// Unwraps the value
    pub fn into_inner(self) -> T {
        self.value.into_inner()
    }
}

struct AbortOnPanic;

impl Drop for AbortOnPanic {
    fn drop(&mut self) {
        if std::thread::panicking() {
            std::process::abort()
        }
    }
}

/// A [`RwLockReadGuard`], without the data
pub(crate) struct RwLockReadGuardDetached<'a, R: RawRwLock> {
    lock: &'a R,
    _marker: PhantomData<R::GuardMarker>,
}

impl<'a, R: RawRwLock> Drop for RwLockReadGuardDetached<'a, R> {
    fn drop(&mut self) {
        unsafe {
            self.lock.unlock_shared();
        }
    }
}

/// A [`RwLockWriteGuard`], without the data
pub(crate) struct RwLockWriteGuardDetached<'a, R: RawRwLock> {
    lock: &'a R,
    _marker: PhantomData<R::GuardMarker>,
}

impl<'a, R: RawRwLock> Drop for RwLockWriteGuardDetached<'a, R> {
    fn drop(&mut self) {
        unsafe {
            self.lock.unlock_exclusive();
        }
    }
}

impl<'a, R: RawRwLock> RwLockReadGuardDetached<'a, R> {
    /// Separates the data from the [`RwLockReadGuard`]
    pub(crate) unsafe fn detach_from<T>(guard: RwLockReadGuard<'a, R, T>) -> (Self, &'a T) {
        let rwlock = RwLockReadGuard::rwlock(&ManuallyDrop::new(guard));

        let data = unsafe { &*rwlock.data_ptr() };
        let guard = unsafe {
            RwLockReadGuardDetached {
                lock: rwlock.raw(),
                _marker: PhantomData,
            }
        };
        (guard, data)
    }
}

impl<'a, R: RawRwLock> RwLockWriteGuardDetached<'a, R> {
    /// Separates the data from the [`RwLockWriteGuard`]
    pub(crate) unsafe fn detach_from<T>(guard: RwLockWriteGuard<'a, R, T>) -> (Self, &'a mut T) {
        let rwlock = RwLockWriteGuard::rwlock(&ManuallyDrop::new(guard));

        let data = unsafe { &mut *rwlock.data_ptr() };
        let guard = unsafe {
            RwLockWriteGuardDetached {
                lock: rwlock.raw(),
                _marker: PhantomData,
            }
        };
        (guard, data)
    }
}

impl<'a, R: RawRwLockDowngrade> RwLockWriteGuardDetached<'a, R> {
    pub(crate) unsafe fn downgrade(self) -> RwLockReadGuardDetached<'a, R> {
        self.lock.downgrade();
        RwLockReadGuardDetached {
            lock: self.lock,
            _marker: self._marker,
        }
    }
}
