use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::ops::Deref;

// NOTE: These types _cannot_ be public, as doing so may cause external implementations to
// implement different behavior for them, which would make transmutes between
// SomeWrapper<Aliased<T, NoDrop>> and SomeWrapper<Aliased<T, DoDrop>> unsound.
pub(crate) struct NoDrop;
pub(crate) struct DoDrop;

pub trait DropBehavior {
    fn do_drop() -> bool;
}
impl DropBehavior for NoDrop {
    fn do_drop() -> bool {
        false
    }
}
impl DropBehavior for DoDrop {
    fn do_drop() -> bool {
        true
    }
}

/// A `T` that is aliased across the two map copies.
///
/// You should be able to mostly ignore this type, as it can generally be treated exactly like a
/// `&T`. However, there are some minor exceptions around forwarding traits -- since `Aliased` is a
/// wrapper type around `T`, it cannot automatically forward traits it does not know about to `&T`.
/// This means that if your `&T` implements, say, `Serialize` or some custom `Borrow<Q>`,
/// `Aliased<T>` will not implement that same trait. You can work around this either by
/// implementing your trait specifically for `Aliased<T>` where possible, or by manually
/// dereferencing to get the `&T` before using the trait.
#[repr(transparent)]
pub struct Aliased<T, U>
where
    U: DropBehavior,
{
    aliased: MaybeUninit<T>,

    drop_behavior: PhantomData<U>,

    // We cannot implement Send just because T is Send since we're aliasing it.
    _no_auto_send: PhantomData<*const T>,
}

impl<T> Aliased<T, NoDrop> {
    /// Create an alias of the inner `T`.
    ///
    /// # Safety
    ///
    /// This method is safe because safe code can only get at `&T`, and cannot cause the `T` to be
    /// dropped. Nevertheless, you should be careful to only invoke this method assuming that you
    /// _do_ call [`drop_copies`] somewhere, as the `Aliased` is no longer safe to use after it is
    /// truly dropped.
    pub(crate) fn alias(&self) -> Self {
        // safety:
        //   We are aliasing T here, but it is okay because:
        //    a) the T is behind a MaybeUninit, and so will cannot be accessed safely; and
        //    b) we only expose _either_ &T while aliased, or &mut after the aliasing ends.
        Aliased {
            aliased: unsafe { std::ptr::read(&self.aliased) },
            drop_behavior: PhantomData,
            _no_auto_send: PhantomData,
        }
    }

    /// Construct an aliased value around a `T`.
    ///
    /// Note that we do not implement `From<T>` because we do not want users to construct
    /// `Aliased<T>`s on their own. If they did, they would almost certain end up with incorrect
    /// drop behavior.
    pub(crate) fn from(t: T) -> Self {
        Self {
            aliased: MaybeUninit::new(t),
            drop_behavior: PhantomData,
            _no_auto_send: PhantomData,
        }
    }

    /// Turn this aliased `T` into one that will drop the `T` when it is dropped.
    ///
    /// # Safety
    ///
    /// This is safe if and only if `self` is the last alias for the `T`.
    pub(crate) unsafe fn dropping(self) -> Aliased<T, DoDrop> {
        Aliased {
            // safety:
            aliased: std::ptr::read(&self.aliased),
            drop_behavior: PhantomData,
            _no_auto_send: PhantomData,
        }
    }
}

// Aliased gives &T across threads if shared or sent across thread boundaries.
// Aliased gives &mut T across threads (for drop) if sent across thread boundaries.
// This implies that we are only Send if T is Send+Sync, and Sync if T is Sync.
//
// Note that these bounds are stricter than what the compiler would auto-generate for the type.
unsafe impl<T, U> Send for Aliased<T, U>
where
    U: DropBehavior,
    T: Send + Sync,
{
}
unsafe impl<T, U> Sync for Aliased<T, U>
where
    U: DropBehavior,
    T: Sync,
{
}

impl<T, U> Drop for Aliased<T, U>
where
    U: DropBehavior,
{
    fn drop(&mut self) {
        if U::do_drop() {
            // safety:
            //   MaybeUninit<T> was created from a valid T.
            //   That T has not been dropped (getting a Aliased<T, DoDrop> is unsafe).
            //   T is no longer aliased (by the safety assumption of getting a Aliased<T, DoDrop>),
            //   so we are allowed to re-take ownership of the T.
            unsafe { std::ptr::drop_in_place(self.aliased.as_mut_ptr()) }
        }
    }
}

impl<T, U> AsRef<T> for Aliased<T, U>
where
    U: DropBehavior,
{
    fn as_ref(&self) -> &T {
        // safety:
        //   MaybeUninit<T> was created from a valid T.
        //   That T has not been dropped (getting a Aliased<T, DoDrop> is unsafe).
        //   All we have done to T is alias it. But, since we only give out &T
        //   (which should be legal anyway), we're fine.
        unsafe { &*self.aliased.as_ptr() }
    }
}

impl<T, U> Deref for Aliased<T, U>
where
    U: DropBehavior,
{
    type Target = T;
    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

use std::hash::{Hash, Hasher};
impl<T, U> Hash for Aliased<T, U>
where
    U: DropBehavior,
    T: Hash,
{
    fn hash<H>(&self, state: &mut H)
    where
        H: Hasher,
    {
        self.as_ref().hash(state)
    }
}

use std::fmt;
impl<T, U> fmt::Debug for Aliased<T, U>
where
    U: DropBehavior,
    T: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_ref().fmt(f)
    }
}

impl<T, U> PartialEq for Aliased<T, U>
where
    U: DropBehavior,
    T: PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        self.as_ref().eq(other.as_ref())
    }

    fn ne(&self, other: &Self) -> bool {
        self.as_ref().ne(other.as_ref())
    }
}

impl<T, U> Eq for Aliased<T, U>
where
    U: DropBehavior,
    T: Eq,
{
}

impl<T, U> PartialOrd for Aliased<T, U>
where
    U: DropBehavior,
    T: PartialOrd,
{
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.as_ref().partial_cmp(other.as_ref())
    }

    fn lt(&self, other: &Self) -> bool {
        self.as_ref().lt(other.as_ref())
    }

    fn le(&self, other: &Self) -> bool {
        self.as_ref().le(other.as_ref())
    }

    fn gt(&self, other: &Self) -> bool {
        self.as_ref().gt(other.as_ref())
    }

    fn ge(&self, other: &Self) -> bool {
        self.as_ref().ge(other.as_ref())
    }
}

impl<T, U> Ord for Aliased<T, U>
where
    U: DropBehavior,
    T: Ord,
{
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_ref().cmp(other.as_ref())
    }
}

use std::borrow::Borrow;
impl<T, U> Borrow<T> for Aliased<T, U>
where
    U: DropBehavior,
{
    fn borrow(&self) -> &T {
        self.as_ref()
    }
}
// What we _really_ want here is:
// ```
// impl<T, U> Borrow<U> for Aliased<T>
// where
//     T: Borrow<U>,
// {
//     fn borrow(&self) -> &U {
//         self.as_ref().borrow()
//     }
// }
// ```
// But unfortunately that won't work due to trait coherence.
// Instead, we manually write the nice Borrow impls from std.
// This won't help with custom Borrow impls, but gets you pretty far.
impl<U> Borrow<str> for Aliased<String, U>
where
    U: DropBehavior,
{
    fn borrow(&self) -> &str {
        self.as_ref()
    }
}
impl<U> Borrow<std::path::Path> for Aliased<std::path::PathBuf, U>
where
    U: DropBehavior,
{
    fn borrow(&self) -> &std::path::Path {
        self.as_ref()
    }
}
impl<T, U> Borrow<[T]> for Aliased<Vec<T>, U>
where
    U: DropBehavior,
{
    fn borrow(&self) -> &[T] {
        self.as_ref()
    }
}
impl<T, U> Borrow<T> for Aliased<Box<T>, U>
where
    T: ?Sized,
    U: DropBehavior,
{
    fn borrow(&self) -> &T {
        self.as_ref()
    }
}
impl<T, U> Borrow<T> for Aliased<std::sync::Arc<T>, U>
where
    T: ?Sized,
    U: DropBehavior,
{
    fn borrow(&self) -> &T {
        self.as_ref()
    }
}
impl<T, U> Borrow<T> for Aliased<std::rc::Rc<T>, U>
where
    T: ?Sized,
    U: DropBehavior,
{
    fn borrow(&self) -> &T {
        self.as_ref()
    }
}
