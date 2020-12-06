//! A lock-free, eventually consistent, concurrent multi-value map.
//!
//! This map implementation allows reads and writes to execute entirely in parallel, with no
//! implicit synchronization overhead. Reads never take locks on their critical path, and neither
//! do writes assuming there is a single writer (multi-writer is possible using a `Mutex`), which
//! significantly improves performance under contention. See the [`left-right` crate](left_right)
//! for details on the underlying concurrency primitive.
//!
//! The trade-off exposed by this type is one of eventual consistency: writes are not visible to
//! readers except following explicit synchronization. Specifically, readers only see the
//! operations that preceeded the last call to `WriteHandle::refresh` by a writer. This lets
//! writers decide how stale they are willing to let reads get. They can refresh the map after
//! every write to emulate a regular concurrent `HashMap`, or they can refresh only occasionally to
//! reduce the synchronization overhead at the cost of stale reads.
//!
//! For read-heavy workloads, the scheme used by this module is particularly useful. Writers can
//! afford to refresh after every write, which provides up-to-date reads, and readers remain fast
//! as they do not need to ever take locks.
//!
//! The map is multi-value, meaning that every key maps to a *collection* of values. This
//! introduces some memory cost by adding a layer of indirection through a `Vec` for each value,
//! but enables more advanced use. This choice was made as it would not be possible to emulate such
//! functionality on top of the semantics of this map (think about it -- what would the operational
//! log contain?).
//!
//! To faciliate more advanced use-cases, each of the two maps also carry some customizeable
//! meta-information. The writers may update this at will, and when a refresh happens, the current
//! meta will also be made visible to readers. This could be useful, for example, to indicate what
//! time the refresh happened.
//!
//! # Examples
//!
//! Single-reader, single-writer
//!
//! ```
//! // new will use the default HashMap hasher, and a meta of ()
//! // note that we get separate read and write handles
//! // the read handle can be cloned to have more readers
//! let (mut book_reviews_w, book_reviews_r) = evmap::new();
//!
//! // review some books.
//! book_reviews_w.insert("Adventures of Huckleberry Finn",    "My favorite book.");
//! book_reviews_w.insert("Grimms' Fairy Tales",               "Masterpiece.");
//! book_reviews_w.insert("Pride and Prejudice",               "Very enjoyable.");
//! book_reviews_w.insert("The Adventures of Sherlock Holmes", "Eye lyked it alot.");
//!
//! // at this point, reads from book_reviews_r will not see any of the reviews!
//! assert_eq!(book_reviews_r.len(), 0);
//! // we need to refresh first to make the writes visible
//! book_reviews_w.publish();
//! assert_eq!(book_reviews_r.len(), 4);
//! // reads will now return Some() because the map has been initialized
//! assert_eq!(book_reviews_r.get("Grimms' Fairy Tales").map(|rs| rs.len()), Some(1));
//!
//! // remember, this is a multi-value map, so we can have many reviews
//! book_reviews_w.insert("Grimms' Fairy Tales",               "Eh, the title seemed weird.");
//! book_reviews_w.insert("Pride and Prejudice",               "Too many words.");
//!
//! // but again, new writes are not yet visible
//! assert_eq!(book_reviews_r.get("Grimms' Fairy Tales").map(|rs| rs.len()), Some(1));
//!
//! // we need to refresh first
//! book_reviews_w.publish();
//! assert_eq!(book_reviews_r.get("Grimms' Fairy Tales").map(|rs| rs.len()), Some(2));
//!
//! // oops, this review has a lot of spelling mistakes, let's delete it.
//! // remove_entry deletes *all* reviews (though in this case, just one)
//! book_reviews_w.remove_entry("The Adventures of Sherlock Holmes");
//! // but again, it's not visible to readers until we refresh
//! assert_eq!(book_reviews_r.get("The Adventures of Sherlock Holmes").map(|rs| rs.len()), Some(1));
//! book_reviews_w.publish();
//! assert_eq!(book_reviews_r.get("The Adventures of Sherlock Holmes").map(|rs| rs.len()), None);
//!
//! // look up the values associated with some keys.
//! let to_find = ["Pride and Prejudice", "Alice's Adventure in Wonderland"];
//! for book in &to_find {
//!     if let Some(reviews) = book_reviews_r.get(book) {
//!         for review in &*reviews {
//!             println!("{}: {}", book, review);
//!         }
//!     } else {
//!         println!("{} is unreviewed.", book);
//!     }
//! }
//!
//! // iterate over everything.
//! for (book, reviews) in &book_reviews_r.enter().unwrap() {
//!     for review in reviews {
//!         println!("{}: \"{}\"", book, review);
//!     }
//! }
//! ```
//!
//! Reads from multiple threads are possible by cloning the `ReadHandle`.
//!
//! ```
//! use std::thread;
//! let (mut book_reviews_w, book_reviews_r) = evmap::new();
//!
//! // start some readers
//! let readers: Vec<_> = (0..4).map(|_| {
//!     let r = book_reviews_r.clone();
//!     thread::spawn(move || {
//!         loop {
//!             let l = r.len();
//!             if l == 0 {
//!                 thread::yield_now();
//!             } else {
//!                 // the reader will either see all the reviews,
//!                 // or none of them, since refresh() is atomic.
//!                 assert_eq!(l, 4);
//!                 break;
//!             }
//!         }
//!     })
//! }).collect();
//!
//! // do some writes
//! book_reviews_w.insert("Adventures of Huckleberry Finn",    "My favorite book.");
//! book_reviews_w.insert("Grimms' Fairy Tales",               "Masterpiece.");
//! book_reviews_w.insert("Pride and Prejudice",               "Very enjoyable.");
//! book_reviews_w.insert("The Adventures of Sherlock Holmes", "Eye lyked it alot.");
//! // expose the writes
//! book_reviews_w.publish();
//!
//! // you can read through the write handle
//! assert_eq!(book_reviews_w.len(), 4);
//!
//! // the original read handle still works too
//! assert_eq!(book_reviews_r.len(), 4);
//!
//! // all the threads should eventually see .len() == 4
//! for r in readers.into_iter() {
//!     assert!(r.join().is_ok());
//! }
//! ```
//!
//! If multiple writers are needed, the `WriteHandle` must be protected by a `Mutex`.
//!
//! ```
//! use std::thread;
//! use std::sync::{Arc, Mutex};
//! let (mut book_reviews_w, book_reviews_r) = evmap::new();
//!
//! // start some writers.
//! // since evmap does not support concurrent writes, we need
//! // to protect the write handle by a mutex.
//! let w = Arc::new(Mutex::new(book_reviews_w));
//! let writers: Vec<_> = (0..4).map(|i| {
//!     let w = w.clone();
//!     thread::spawn(move || {
//!         let mut w = w.lock().unwrap();
//!         w.insert(i, true);
//!         w.publish();
//!     })
//! }).collect();
//!
//! // eventually we should see all the writes
//! while book_reviews_r.len() < 4 { thread::yield_now(); };
//!
//! // all the threads should eventually finish writing
//! for w in writers.into_iter() {
//!     assert!(w.join().is_ok());
//! }
//! ```
//!
//! [`ReadHandle`] is not `Sync` as sharing a single instance amongst threads would introduce a
//! significant performance bottleneck. A fresh `ReadHandle` needs to be created for each thread
//! either by cloning a [`ReadHandle`] or from a [`ReadHandleFactory`]. For further information,
//! see [`left_right::ReadHandle`].
//!
//! # Implementation
//!
//! Under the hood, the map is implemented using two regular `HashMap`s and some magic. Take a look
//! at [`left-right`](left_right) for a much more in-depth discussion. Since the implementation
//! uses regular `HashMap`s under the hood, table resizing is fully supported. It does, however,
//! also mean that the memory usage of this implementation is approximately twice of that of a
//! regular `HashMap`, and more if writers rarely refresh after writing.
//!
//! # Value storage
//!
//! The values for each key in the map are stored in [`Values`]. Conceptually, each `Values` is a
//! _bag_ or _multiset_; it can store multiple copies of the same value. `evmap` applies some
//! cleverness in an attempt to reduce unnecessary allocations and keep the cost of operations on
//! even large value-bags small. For small bags, `Values` uses the `smallvec` crate. This avoids
//! allocation entirely for single-element bags, and uses a `Vec` if the bag is relatively small.
//! For large bags, `Values` uses the `hashbag` crate, which enables `evmap` to efficiently look up
//! and remove specific elements in the value bag. For bags larger than one element, but smaller
//! than the threshold for moving to `hashbag`, we use `smallvec` to avoid unnecessary hashing.
//! Operations such as `Fit` and `Replace` will automatically switch back to the inline storage if
//! possible. This is ideal for maps that mostly use one element per key, as it can improvate
//! memory locality with less indirection.
#![warn(
    missing_docs,
    rust_2018_idioms,
    missing_debug_implementations,
    broken_intra_doc_links
)]
#![allow(clippy::type_complexity)]
// This _should_ detect if we ever accidentally leak aliasing::NoDrop.
// But, currently, it does not..
#![deny(unreachable_pub)]

use left_right::aliasing::Aliased;
use std::collections::hash_map::RandomState;
use std::fmt;
use std::hash::{BuildHasher, Hash};

mod inner;
use crate::inner::Inner;

mod values;
pub use values::Values;

mod write;
pub use crate::write::WriteHandle;

mod read;
// These cannot use ::{..} syntax because of
// https://github.com/rust-lang/rust/issues/57411
pub use crate::read::ReadHandle;
pub use crate::read::ReadHandleFactory;

/// Helper types that give access to values inside the read half of an `evmap`.
pub mod refs {
    // Same here, ::{..} won't work.
    pub use crate::read::MapReadRef;
    pub use crate::read::ReadGuardIter;
}

// NOTE: It is _critical_ that this module is not public.
mod aliasing;

// Expose `ReadGuard` since it has useful methods the user will likely care about.
#[doc(inline)]
pub use left_right::ReadGuard;

/// Unary predicate used to retain elements.
///
/// The predicate function is called once for each distinct value, and `true` if this is the
/// _first_ call to the predicate on the _second_ application of the operation.
pub struct Predicate<V: ?Sized>(pub(crate) Box<dyn FnMut(&V, bool) -> bool + Send>);

impl<V: ?Sized> Predicate<V> {
    /// Evaluate the predicate for the given element
    #[inline]
    pub fn eval(&mut self, value: &V, reset: bool) -> bool {
        (*self.0)(value, reset)
    }
}

impl<V: ?Sized> PartialEq for Predicate<V> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        // only compare data, not vtable: https://stackoverflow.com/q/47489449/472927
        &*self.0 as *const _ as *const () == &*other.0 as *const _ as *const ()
    }
}

impl<V: ?Sized> Eq for Predicate<V> {}

impl<V: ?Sized> fmt::Debug for Predicate<V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Predicate")
            .field(&format_args!("{:p}", &*self.0 as *const _))
            .finish()
    }
}

/// A pending map operation.
#[non_exhaustive]
pub(crate) enum Operation<K, V, M> {
    /// Replace the set of entries for this key with this value.
    Replace(K, Aliased<V, crate::aliasing::NoDrop>),
    /// Add this value to the set of entries for this key.
    Add(K, Aliased<V, crate::aliasing::NoDrop>),
    /// Remove this value from the set of entries for this key.
    RemoveValue(K, V),
    /// Remove the value set for this key.
    RemoveEntry(K),
    #[cfg(feature = "eviction")]
    /// Drop keys at the given indices.
    ///
    /// The list of indices must be sorted in ascending order.
    EmptyAt(Vec<usize>),
    /// Remove all values in the value set for this key.
    Clear(K),
    /// Remove all values for all keys.
    ///
    /// Note that this will iterate once over all the keys internally.
    Purge,
    /// Retains all values matching the given predicate.
    Retain(K, Predicate<V>),
    /// Shrinks [`Values`] to their minimum necessary size, freeing memory
    /// and potentially improving cache locality.
    ///
    /// If no key is given, all `Values` will shrink to fit.
    Fit(Option<K>),
    /// Reserves capacity for some number of additional elements in [`Values`]
    /// for the given key. If the given key does not exist, allocate an empty
    /// `Values` with the given capacity.
    ///
    /// This can improve performance by pre-allocating space for large bags of values.
    Reserve(K, usize),
    /// Mark the map as ready to be consumed for readers.
    MarkReady,
    /// Set the value of the map meta.
    SetMeta(M),
    /// Copy over the contents of the read map wholesale as the write map is empty.
    JustCloneRHandle,
}

impl<K, V, M> fmt::Debug for Operation<K, V, M>
where
    K: fmt::Debug,
    V: fmt::Debug,
    M: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Operation::Replace(ref a, ref b) => f.debug_tuple("Replace").field(a).field(b).finish(),
            Operation::Add(ref a, ref b) => f.debug_tuple("Add").field(a).field(b).finish(),
            Operation::RemoveValue(ref a, ref b) => {
                f.debug_tuple("RemoveValue").field(a).field(b).finish()
            }
            Operation::RemoveEntry(ref a) => f.debug_tuple("RemoveEntry").field(a).finish(),
            #[cfg(feature = "eviction")]
            Operation::EmptyAt(ref a) => f.debug_tuple("EmptyAt").field(a).finish(),
            Operation::Clear(ref a) => f.debug_tuple("Clear").field(a).finish(),
            Operation::Purge => f.debug_tuple("Purge").finish(),
            Operation::Retain(ref a, ref b) => f.debug_tuple("Retain").field(a).field(b).finish(),
            Operation::Fit(ref a) => f.debug_tuple("Fit").field(a).finish(),
            Operation::Reserve(ref a, ref b) => f.debug_tuple("Reserve").field(a).field(b).finish(),
            Operation::MarkReady => f.debug_tuple("MarkReady").finish(),
            Operation::SetMeta(ref a) => f.debug_tuple("SetMeta").field(a).finish(),
            Operation::JustCloneRHandle => f.debug_tuple("JustCloneRHandle").finish(),
        }
    }
}

/// Options for how to initialize the map.
///
/// In particular, the options dictate the hashing function, meta type, and initial capacity of the
/// map.
pub struct Options<M, S>
where
    S: BuildHasher,
{
    meta: M,
    hasher: S,
    capacity: Option<usize>,
}

impl<M, S> fmt::Debug for Options<M, S>
where
    S: BuildHasher,
    M: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Options")
            .field("meta", &self.meta)
            .field("capacity", &self.capacity)
            .finish()
    }
}

impl Default for Options<(), RandomState> {
    fn default() -> Self {
        Options {
            meta: (),
            hasher: RandomState::default(),
            capacity: None,
        }
    }
}

impl<M, S> Options<M, S>
where
    S: BuildHasher,
{
    /// Set the initial meta value for the map.
    pub fn with_meta<M2>(self, meta: M2) -> Options<M2, S> {
        Options {
            meta,
            hasher: self.hasher,
            capacity: self.capacity,
        }
    }

    /// Set the hasher used for the map.
    pub fn with_hasher<S2>(self, hash_builder: S2) -> Options<M, S2>
    where
        S2: BuildHasher,
    {
        Options {
            meta: self.meta,
            hasher: hash_builder,
            capacity: self.capacity,
        }
    }

    /// Set the initial capacity for the map.
    pub fn with_capacity(self, capacity: usize) -> Options<M, S> {
        Options {
            meta: self.meta,
            hasher: self.hasher,
            capacity: Some(capacity),
        }
    }

    /// Create the map, and construct the read and write handles used to access it.
    #[allow(clippy::type_complexity)]
    pub fn construct<K, V>(self) -> (WriteHandle<K, V, M, S>, ReadHandle<K, V, M, S>)
    where
        K: Eq + Hash + Clone,
        S: BuildHasher + Clone,
        V: Eq + Hash,
        M: 'static + Clone,
    {
        let inner = if let Some(cap) = self.capacity {
            Inner::with_capacity_and_hasher(self.meta, cap, self.hasher)
        } else {
            Inner::with_hasher(self.meta, self.hasher)
        };

        let (mut w, r) = left_right::new_from_empty(inner);
        w.append(Operation::MarkReady);

        (WriteHandle::new(w), ReadHandle::new(r))
    }
}

/// Create an empty eventually consistent map.
///
/// Use the [`Options`](./struct.Options.html) builder for more control over initialization.
#[allow(clippy::type_complexity)]
pub fn new<K, V>() -> (
    WriteHandle<K, V, (), RandomState>,
    ReadHandle<K, V, (), RandomState>,
)
where
    K: Eq + Hash + Clone,
    V: Eq + Hash,
{
    Options::default().construct()
}

/// Create an empty eventually consistent map with meta information.
///
/// Use the [`Options`](./struct.Options.html) builder for more control over initialization.
#[allow(clippy::type_complexity)]
pub fn with_meta<K, V, M>(
    meta: M,
) -> (
    WriteHandle<K, V, M, RandomState>,
    ReadHandle<K, V, M, RandomState>,
)
where
    K: Eq + Hash + Clone,
    V: Eq + Hash,
    M: 'static + Clone,
{
    Options::default().with_meta(meta).construct()
}

/// Create an empty eventually consistent map with meta information and custom hasher.
///
/// Use the [`Options`](./struct.Options.html) builder for more control over initialization.
#[allow(clippy::type_complexity)]
pub fn with_hasher<K, V, M, S>(
    meta: M,
    hasher: S,
) -> (WriteHandle<K, V, M, S>, ReadHandle<K, V, M, S>)
where
    K: Eq + Hash + Clone,
    V: Eq + Hash,
    M: 'static + Clone,
    S: BuildHasher + Clone,
{
    Options::default()
        .with_hasher(hasher)
        .with_meta(meta)
        .construct()
}
