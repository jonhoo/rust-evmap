use super::Absorb;
use crate::read::ReadHandle;

use std::collections::VecDeque;
use std::ptr::NonNull;
use std::sync::atomic;
#[cfg(test)]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, MutexGuard};
use std::{fmt, thread};

/// A writer handle to a left-right guarded data structure.
///
/// All operations on the underlying data should be enqueued as operations of type `O` using
/// [`append`](Self::append). The effect of this operations are only exposed to readers once
/// [`publish`](Self::publish) is called.
///
/// # Reading through a `WriteHandle`
///
/// `WriteHandle` allows access to a [`ReadHandle`] through `Deref<Target = ReadHandle>`. Note that
/// since the reads go through a [`ReadHandle`], those reads are subject to the same visibility
/// restrictions as reads that do not go through the `WriteHandle`: they only see the effects of
/// operations prior to the last call to [`publish`](Self::publish).
pub struct WriteHandle<T, O>
where
    T: Absorb<O>,
{
    epochs: crate::Epochs,
    w_handle: NonNull<T>,
    oplog: VecDeque<O>,
    swap_index: usize,
    r_handle: ReadHandle<T>,
    last_epochs: Vec<usize>,
    #[cfg(test)]
    refreshes: usize,
    #[cfg(test)]
    is_waiting: Arc<AtomicBool>,
    /// Write directly to the write handle map, since no publish has happened.
    first: bool,
    /// A publish has happened, but the two copies have not been synchronized yet.
    second: bool,
}

// safety: if a `WriteHandle` is sent across a thread boundary, we need to be able to take
// ownership of both Ts and Os across that thread boundary. since `WriteHandle` holds a
// `ReadHandle`, we also need to respect its Send requirements.
unsafe impl<T, O> Send for WriteHandle<T, O>
where
    T: Absorb<O>,
    T: Send,
    O: Send,
    ReadHandle<T>: Send,
{
}

impl<T, O> fmt::Debug for WriteHandle<T, O>
where
    T: Absorb<O> + fmt::Debug,
    O: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WriteHandle")
            .field("epochs", &self.epochs)
            .field("w_handle", &self.w_handle)
            .field("oplog", &self.oplog)
            .field("swap_index", &self.swap_index)
            .field("r_handle", &self.r_handle)
            .field("first", &self.first)
            .field("second", &self.second)
            .finish()
    }
}

impl<T, O> Drop for WriteHandle<T, O>
where
    T: Absorb<O>,
{
    fn drop(&mut self) {
        use std::ptr;

        // first, ensure both copies are up to date
        // (otherwise safely dropping deduplicated data is a pain)
        if !self.oplog.is_empty() {
            self.publish();
        }
        if !self.oplog.is_empty() {
            self.publish();
        }
        assert!(self.oplog.is_empty());

        // next, grab the read handle and set it to NULL
        let r_handle = self.r_handle.inner.swap(ptr::null_mut(), Ordering::Release);

        // now, wait for all readers to depart
        let epochs = Arc::clone(&self.epochs);
        let mut epochs = epochs.lock().unwrap();
        self.wait(&mut epochs);

        // ensure that the subsequent epoch reads aren't re-ordered to before the swap
        atomic::fence(Ordering::SeqCst);

        // all readers have now observed the NULL, so we own both handles.
        // all operations have been applied to both w_handle and r_handle.
        // give the underlying data structure an opportunity to handle the one copy differently:
        //
        // safety: w_handle was initially crated from a `Box`, and is no longer aliased.
        Absorb::drop_first(unsafe { Box::from_raw(self.w_handle.as_ptr()) });

        // next we drop the r_handle.
        // this is safe, since we know that no readers are using this pointer
        // anymore (due to the .wait() following swapping the pointer with NULL).
        //
        // safety: r_handle was initially crated from a `Box`, and is no longer aliased.
        Absorb::drop_second(unsafe { Box::from_raw(r_handle) });
    }
}

impl<T, O> WriteHandle<T, O>
where
    T: Absorb<O>,
{
    pub(crate) fn new(w_handle: T, epochs: crate::Epochs, r_handle: ReadHandle<T>) -> Self {
        Self {
            epochs,
            // safety: Box<T> is not null and covariant.
            w_handle: unsafe { NonNull::new_unchecked(Box::into_raw(Box::new(w_handle))) },
            oplog: VecDeque::new(),
            swap_index: 0,
            r_handle,
            last_epochs: Vec::new(),
            #[cfg(test)]
            is_waiting: Arc::new(AtomicBool::new(false)),
            #[cfg(test)]
            refreshes: 0,
            first: true,
            second: true,
        }
    }

    fn wait(&mut self, epochs: &mut MutexGuard<'_, slab::Slab<Arc<AtomicUsize>>>) {
        let mut iter = 0;
        let mut starti = 0;

        #[cfg(test)]
        {
            self.is_waiting.swap(true, Ordering::Relaxed);
        }
        // we're over-estimating here, but slab doesn't expose its max index
        self.last_epochs.resize(epochs.capacity(), 0);
        'retry: loop {
            // read all and see if all have changed (which is likely)
            for (ii, (ri, epoch)) in epochs.iter().enumerate().skip(starti) {
                // if the reader's epoch was even last we read it (which was _after_ the swap),
                // then they either do not have the pointer, or must have read the pointer strictly
                // after the swap. in either case, they cannot be using the old pointer value (what
                // is now w_handle).
                //
                // note that this holds even with wrap-around since std::u{N}::MAX == 2 ^ N - 1,
                // which is odd, and std::u{N}::MAX + 1 == 0 is even.
                //
                // note also that `ri` _may_ have been re-used since we last read into last_epochs.
                // this is okay though, as a change still implies that the new reader must have
                // arrived _after_ we did the atomic swap, and thus must also have seen the new
                // pointer.
                if self.last_epochs[ri] % 2 == 0 {
                    continue;
                }

                let now = epoch.load(Ordering::Acquire);
                if now != self.last_epochs[ri] {
                    // reader must have seen the last swap, since they have done at least one
                    // operation since we last looked at their epoch, which _must_ mean that they
                    // are no longer using the old pointer value.
                } else {
                    // reader may not have seen swap
                    // continue from this reader's epoch
                    starti = ii;

                    // how eagerly should we retry?
                    if iter != 20 {
                        iter += 1;
                    } else {
                        thread::yield_now();
                    }

                    continue 'retry;
                }
            }
            break;
        }
        #[cfg(test)]
        {
            self.is_waiting.swap(false, Ordering::Relaxed);
        }
    }

    /// Publish all operations append to the log to reads.
    ///
    /// This method needs to wait for all readers to move to the "other" copy of the data so that
    /// it can replay the operational log onto the stale copy the readers used to use. This can
    /// take some time, especially if readers are executing slow operations, or if there are many
    /// of them.
    pub fn publish(&mut self) -> &mut Self {
        // we need to wait until all epochs have changed since the swaps *or* until a "finished"
        // flag has been observed to be on for two subsequent iterations (there still may be some
        // readers present since we did the previous refresh)
        //
        // NOTE: it is safe for us to hold the lock for the entire duration of the swap. we will
        // only block on pre-existing readers, and they are never waiting to push onto epochs
        // unless they have finished reading.
        let epochs = Arc::clone(&self.epochs);
        let mut epochs = epochs.lock().unwrap();

        self.wait(&mut epochs);

        if !self.first {
            // all the readers have left!
            // safety: we haven't freed the Box, and no readers are accessing the w_handle
            let w_handle = unsafe { self.w_handle.as_mut() };

            // safety: we will not swap while we hold this reference
            let r_handle = unsafe {
                self.r_handle
                    .inner
                    .load(Ordering::Acquire)
                    .as_ref()
                    .unwrap()
            };

            if self.second {
                Absorb::sync_with(w_handle, r_handle);
                self.second = false
            }

            // the w_handle copy has not seen any of the writes in the oplog
            // the r_handle copy has not seen any of the writes following swap_index
            if self.swap_index != 0 {
                // we can drain out the operations that only the w_handle copy needs
                //
                // NOTE: the if above is because drain(0..0) would remove 0
                for op in self.oplog.drain(0..self.swap_index) {
                    T::absorb_second(w_handle, op, r_handle);
                }
            }
            // we cannot give owned operations to absorb_first
            // since they'll also be needed by the r_handle copy
            for op in self.oplog.iter_mut() {
                T::absorb_first(w_handle, op, r_handle);
            }
            // the w_handle copy is about to become the r_handle, and can ignore the oplog
            self.swap_index = self.oplog.len();

        // w_handle (the old r_handle) is now fully up to date!
        } else {
            self.first = false
        }

        // at this point, we have exclusive access to w_handle, and it is up-to-date with all
        // writes. the stale r_handle is accessed by readers through an Arc clone of atomic pointer
        // inside the ReadHandle. oplog contains all the changes that are in w_handle, but not in
        // r_handle.
        //
        // it's now time for us to swap the copies so that readers see up-to-date results from
        // w_handle.

        // swap in our w_handle, and get r_handle in return
        let r_handle = self
            .r_handle
            .inner
            .swap(self.w_handle.as_ptr(), Ordering::Release);

        // NOTE: at this point, there are likely still readers using r_handle.
        // safety: r_handle was also created from a Box, so it is not null and is covariant.
        self.w_handle = unsafe { NonNull::new_unchecked(r_handle) };

        // ensure that the subsequent epoch reads aren't re-ordered to before the swap
        atomic::fence(Ordering::SeqCst);

        for (ri, epoch) in epochs.iter() {
            self.last_epochs[ri] = epoch.load(Ordering::Acquire);
        }

        #[cfg(test)]
        {
            self.refreshes += 1;
        }

        self
    }

    /// Publish as necessary to ensure that all operations are visible to readers.
    ///
    /// `WriteHandle::publish` will *always* wait for old readers to depart and swap the maps.
    /// This method will only do so if there are pending operations.
    pub fn flush(&mut self) {
        if self.has_pending_operations() {
            self.publish();
        }
    }

    /// Returns true if there are operations in the operational log that have not yet been exposed
    /// to readers.
    pub fn has_pending_operations(&self) -> bool {
        // NOTE: we don't use self.oplog.is_empty() here because it's not really that important if
        // there are operations that have not yet been applied to the _write_ handle.
        self.swap_index < self.oplog.len()
    }

    /// Append the given operation to the operational log.
    ///
    /// Its effects will not be exposed to readers until you call [`publish`](Self::publish).
    pub fn append(&mut self, op: O) -> &mut Self {
        self.extend(std::iter::once(op));
        self
    }

    /// Returns a raw pointer to the write copy of the data (the one readers are _not_ accessing).
    ///
    /// Note that it is only safe to mutate through this pointer if you _know_ that there are no
    /// readers still present in this copy. This is not normally something you know; even after
    /// calling `publish`, readers may still be in the write copy for some time. In general, the
    /// only time you know this is okay is before the first call to `publish` (since no readers
    /// ever entered the write copy).
    // TODO: Make this return `Option<&mut T>`,
    // and only `Some` if there are indeed to readers in the write copy.
    pub fn raw_write_handle(&mut self) -> NonNull<T> {
        self.w_handle
    }
}

// allow using write handle for reads
use std::ops::Deref;
impl<T, O> Deref for WriteHandle<T, O>
where
    T: Absorb<O>,
{
    type Target = ReadHandle<T>;
    fn deref(&self) -> &Self::Target {
        &self.r_handle
    }
}

impl<T, O> Extend<O> for WriteHandle<T, O>
where
    T: Absorb<O>,
{
    /// Add multiple operations to the operational log.
    ///
    /// Their effects will not be exposed to readers until you call [`publish`](Self::publish)
    fn extend<I>(&mut self, ops: I)
    where
        I: IntoIterator<Item = O>,
    {
        if self.first {
            // Safety: we know there are no outstanding w_handle readers, since we haven't
            // refreshed ever before, so we can modify it directly!
            let mut w_inner = self.raw_write_handle();
            let w_inner = unsafe { w_inner.as_mut() };
            let r_handle = self.enter().expect("map has not yet been destroyed");
            // Because we are operating directly on the map, and nothing is aliased, we do want
            // to perform drops, so we invoke absorb_second.
            for op in ops {
                Absorb::absorb_second(w_inner, op, &*r_handle);
            }
        } else {
            self.oplog.extend(ops);
        }
    }
}

/// `WriteHandle` can be sent across thread boundaries:
///
/// ```
/// use left_right::WriteHandle;
///
/// struct Data;
/// impl left_right::Absorb<()> for Data {
///     fn absorb_first(&mut self, _: &mut (), _: &Self) {}
///     fn sync_with(&mut self, _: &Self) {}
/// }
///
/// fn is_send<T: Send>() {
///   // dummy function just used for its parameterized type bound
/// }
///
/// is_send::<WriteHandle<Data, ()>>()
/// ```
///
/// As long as the inner types allow that of course.
/// Namely, the data type has to be `Send`:
///
/// ```compile_fail
/// use left_right::WriteHandle;
/// use std::rc::Rc;
///
/// struct Data(Rc<()>);
/// impl left_right::Absorb<()> for Data {
///     fn absorb_first(&mut self, _: &mut (), _: &Self) {}
/// }
///
/// fn is_send<T: Send>() {
///   // dummy function just used for its parameterized type bound
/// }
///
/// is_send::<WriteHandle<Data, ()>>()
/// ```
///
/// .. the operation type has to be `Send`:
///
/// ```compile_fail
/// use left_right::WriteHandle;
/// use std::rc::Rc;
///
/// struct Data;
/// impl left_right::Absorb<Rc<()>> for Data {
///     fn absorb_first(&mut self, _: &mut Rc<()>, _: &Self) {}
/// }
///
/// fn is_send<T: Send>() {
///   // dummy function just used for its parameterized type bound
/// }
///
/// is_send::<WriteHandle<Data, Rc<()>>>()
/// ```
///
/// .. and the data type has to be `Sync` so it's still okay to read through `ReadHandle`s:
///
/// ```compile_fail
/// use left_right::WriteHandle;
/// use std::cell::Cell;
///
/// struct Data(Cell<()>);
/// impl left_right::Absorb<()> for Data {
///     fn absorb_first(&mut self, _: &mut (), _: &Self) {}
/// }
///
/// fn is_send<T: Send>() {
///   // dummy function just used for its parameterized type bound
/// }
///
/// is_send::<WriteHandle<Data, ()>>()
/// ```
#[allow(dead_code)]
struct CheckWriteHandleSend;

#[cfg(test)]
mod tests {
    use crate::CounterAddOp;
    use slab::Slab;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    #[test]
    fn append_test() {
        let (mut w, _r) = crate::new::<i32, _>();
        assert_eq!(w.first, true);
        w.append(CounterAddOp(1));
        assert_eq!(w.oplog.len(), 0);
        assert_eq!(w.first, true);
        w.publish();
        assert_eq!(w.first, false);
        w.append(CounterAddOp(2));
        w.append(CounterAddOp(3));
        assert_eq!(w.oplog.len(), 2);
    }

    #[test]
    fn wait_test() {
        use std::sync::{Arc, Barrier};
        use std::thread;
        let (mut w, _r) = crate::new::<i32, _>();

        // Case 1: If epoch is set to default.
        let test_epochs: crate::Epochs = Default::default();
        let mut test_epochs = test_epochs.lock().unwrap();
        // since there is no epoch to waiting for, wait function will return immediately.
        w.wait(&mut test_epochs);

        // Case 2: If one of the reader is still reading(epoch is odd and count is same as in last_epoch)
        // and wait has been called.
        let held_epoch = Arc::new(AtomicUsize::new(1));

        w.last_epochs = vec![2, 2, 1];
        let mut epochs_slab = Slab::new();
        epochs_slab.insert(Arc::new(AtomicUsize::new(2)));
        epochs_slab.insert(Arc::new(AtomicUsize::new(2)));
        epochs_slab.insert(Arc::clone(&held_epoch));

        let barrier = Arc::new(Barrier::new(2));

        let is_waiting = Arc::clone(&w.is_waiting);

        // check for writer waiting state before calling wait.
        let is_waiting_v = is_waiting.load(Ordering::Relaxed);
        assert_eq!(false, is_waiting_v);

        let barrier2 = Arc::clone(&barrier);
        let test_epochs = Arc::new(Mutex::new(epochs_slab));
        let wait_handle = thread::spawn(move || {
            barrier2.wait();
            let mut test_epochs = test_epochs.lock().unwrap();
            w.wait(&mut test_epochs);
        });

        barrier.wait();

        // make sure that writer wait() will call first only then update the held epoch.
        'retry: loop {
            thread::yield_now();
            let is_waiting_v = is_waiting.load(Ordering::Relaxed);
            if is_waiting_v {
                thread::yield_now();
                break 'retry;
            }
        }

        let is_waiting_v = is_waiting.load(Ordering::Relaxed);
        // check if is_waiting is set to true after calling wait.
        assert_eq!(true, is_waiting_v);

        held_epoch.fetch_add(1, Ordering::SeqCst);

        // join to make sure that wait fn must return after the progress/increment
        // of held_epoch.
        let _ = wait_handle.join();

        let count = held_epoch.load(Ordering::Relaxed);
        assert_eq!(count, 2);

        let is_waiting_v = is_waiting.load(Ordering::SeqCst);
        assert_eq!(false, is_waiting_v);
    }

    #[test]
    fn flush_noblock() {
        let (mut w, r) = crate::new::<i32, _>();
        w.append(CounterAddOp(42));
        w.publish();
        assert_eq!(*r.enter().unwrap(), 42);

        // pin the epoch
        let _count = r.enter();
        // refresh would hang here
        assert_eq!(w.oplog.iter().skip(w.swap_index).count(), 0);
        assert!(!w.has_pending_operations());
    }

    #[test]
    fn flush_no_refresh() {
        let (mut w, _) = crate::new::<i32, _>();

        // Until we refresh, writes are written directly instead of going to the
        // oplog (because there can't be any readers on the w_handle table).
        assert!(!w.has_pending_operations());
        w.publish();
        assert!(!w.has_pending_operations());
        assert_eq!(w.refreshes, 1);

        w.append(CounterAddOp(42));
        assert!(w.has_pending_operations());
        w.publish();
        assert!(!w.has_pending_operations());
        assert_eq!(w.refreshes, 2);

        w.append(CounterAddOp(42));
        assert!(w.has_pending_operations());
        w.publish();
        assert!(!w.has_pending_operations());
        assert_eq!(w.refreshes, 3);

        // Sanity check that a refresh would have been visible
        assert!(!w.has_pending_operations());
        w.publish();
        assert_eq!(w.refreshes, 4);
    }
}
