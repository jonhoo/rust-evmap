//! Types that can be cheaply aliased.

use std::mem::ManuallyDrop;
use std::ops::{Deref, DerefMut};

/// Types that implement this trait can be cheaply copied by (potentially) aliasing the data they
/// contain. Only the _last_ shallow copy will be dropped -- all others will be silently leaked
/// (with `mem::forget`).
///
/// To implement this trait for your own `Copy` type, write:
///
/// ```rust
/// # use evmap::ShallowCopy;
/// use std::mem::ManuallyDrop;
///
/// #[derive(Copy, Clone)]
/// struct T;
///
/// impl ShallowCopy for T {
///     unsafe fn shallow_copy(&self) -> ManuallyDrop<Self> {
///         ManuallyDrop::new(*self)
///     }
/// }
/// ```
///
/// If you have a non-`Copy` type, the value returned by `shallow_copy` should point to the same
/// data as the `&mut self`, and it should be safe to `mem::forget` either of the copies as long as
/// the other is dropped normally afterwards.
///
/// For complex, non-`Copy` types, you can place the type behind a wrapper that implements
/// `ShallowCopy` such as `Box` or `Arc`.
/// Alternatively, if your type is made up of types that all implement `ShallowCopy`, consider
/// using the `evmap-derive` crate, which contains a derive macro for `ShallowCopy`.
/// See that crate's documentation for details.
pub trait ShallowCopy {
    /// Perform an aliasing copy of this value.
    ///
    /// # Safety
    ///
    /// The use of this method is *only* safe if the values involved are never mutated, and only
    /// one of the copies is dropped; the remaining copies must be forgotten with `mem::forget`.
    unsafe fn shallow_copy(&self) -> ManuallyDrop<Self>;
}

use std::sync::Arc;
impl<T> ShallowCopy for Arc<T>
where
    T: ?Sized,
{
    unsafe fn shallow_copy(&self) -> ManuallyDrop<Self> {
        ManuallyDrop::new(Arc::from_raw(&**self as *const _))
    }
}

use std::rc::Rc;
impl<T> ShallowCopy for Rc<T>
where
    T: ?Sized,
{
    unsafe fn shallow_copy(&self) -> ManuallyDrop<Self> {
        ManuallyDrop::new(Rc::from_raw(&**self as *const _))
    }
}

impl<T> ShallowCopy for Box<T>
where
    T: ?Sized,
{
    unsafe fn shallow_copy(&self) -> ManuallyDrop<Self> {
        ManuallyDrop::new(Box::from_raw(&**self as *const _ as *mut _))
    }
}

impl<T> ShallowCopy for Option<T>
where
    T: ShallowCopy,
{
    unsafe fn shallow_copy(&self) -> ManuallyDrop<Self> {
        ManuallyDrop::new(if let Some(value) = self {
            Some(ManuallyDrop::into_inner(value.shallow_copy()))
        } else {
            None
        })
    }
}

impl ShallowCopy for String {
    unsafe fn shallow_copy(&self) -> ManuallyDrop<Self> {
        let buf = self.as_bytes().as_ptr();
        let len = self.len();
        let cap = self.capacity();
        ManuallyDrop::new(String::from_raw_parts(buf as *mut _, len, cap))
    }
}

impl<T> ShallowCopy for Vec<T> {
    unsafe fn shallow_copy(&self) -> ManuallyDrop<Self> {
        let ptr = self.as_ptr() as *mut _;
        let len = self.len();
        let cap = self.capacity();
        ManuallyDrop::new(Vec::from_raw_parts(ptr, len, cap))
    }
}

#[cfg(feature = "bytes")]
impl ShallowCopy for bytes::Bytes {
    unsafe fn shallow_copy(&self) -> ManuallyDrop<Self> {
        let len = self.len();
        let buf: &'static [u8] = std::slice::from_raw_parts(self.as_ptr(), len);
        ManuallyDrop::new(bytes::Bytes::from_static(buf))
    }
}

impl<'a, T> ShallowCopy for &'a T
where
    T: ?Sized,
{
    unsafe fn shallow_copy(&self) -> ManuallyDrop<Self> {
        ManuallyDrop::new(&*self)
    }
}

/// If you are willing to have your values be copied between the two views of the `evmap`,
/// wrap them in this type.
///
/// This is effectively a way to bypass the `ShallowCopy` optimization.
/// Note that you do not need this wrapper for most `Copy` primitives.
#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq, Ord, PartialOrd, Default)]
#[repr(transparent)]
pub struct CopyValue<T>(T);

impl<T: Copy> From<T> for CopyValue<T> {
    fn from(t: T) -> Self {
        CopyValue(t)
    }
}

impl<T> ShallowCopy for CopyValue<T>
where
    T: Copy,
{
    unsafe fn shallow_copy(&self) -> ManuallyDrop<Self> {
        ManuallyDrop::new(CopyValue(self.0))
    }
}

impl<T> Deref for CopyValue<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for CopyValue<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

macro_rules! impl_shallow_copy_for_copy_primitives {
    ($($t:ty)*) => ($(
        impl ShallowCopy for $t {
            unsafe fn shallow_copy(&self) -> ManuallyDrop<Self> {
                ManuallyDrop::new(*self)
            }
        }
    )*)
}

impl_shallow_copy_for_copy_primitives!(() bool char usize u8 u16 u32 u64 u128 isize i8 i16 i32 i64 i128 f32 f64);

macro_rules! tuple_impls {
    ($(
        $Tuple:ident {
            $(($idx:tt) -> $T:ident)+
        }
    )+) => {
        $(
            impl<$($T:ShallowCopy),+> ShallowCopy for ($($T,)+) {
                unsafe fn shallow_copy(&self) -> ManuallyDrop<Self> {
                    ManuallyDrop::new(($(ManuallyDrop::into_inner(self.$idx.shallow_copy()),)+))
                }
            }
        )+
    }
}

tuple_impls! {
    Tuple1 {
        (0) -> A
    }
    Tuple2 {
        (0) -> A
        (1) -> B
    }
    Tuple3 {
        (0) -> A
        (1) -> B
        (2) -> C
    }
    Tuple4 {
        (0) -> A
        (1) -> B
        (2) -> C
        (3) -> D
    }
    Tuple5 {
        (0) -> A
        (1) -> B
        (2) -> C
        (3) -> D
        (4) -> E
    }
    Tuple6 {
        (0) -> A
        (1) -> B
        (2) -> C
        (3) -> D
        (4) -> E
        (5) -> F
    }
    Tuple7 {
        (0) -> A
        (1) -> B
        (2) -> C
        (3) -> D
        (4) -> E
        (5) -> F
        (6) -> G
    }
    Tuple8 {
        (0) -> A
        (1) -> B
        (2) -> C
        (3) -> D
        (4) -> E
        (5) -> F
        (6) -> G
        (7) -> H
    }
    Tuple9 {
        (0) -> A
        (1) -> B
        (2) -> C
        (3) -> D
        (4) -> E
        (5) -> F
        (6) -> G
        (7) -> H
        (8) -> I
    }
    Tuple10 {
        (0) -> A
        (1) -> B
        (2) -> C
        (3) -> D
        (4) -> E
        (5) -> F
        (6) -> G
        (7) -> H
        (8) -> I
        (9) -> J
    }
    Tuple11 {
        (0) -> A
        (1) -> B
        (2) -> C
        (3) -> D
        (4) -> E
        (5) -> F
        (6) -> G
        (7) -> H
        (8) -> I
        (9) -> J
        (10) -> K
    }
    Tuple12 {
        (0) -> A
        (1) -> B
        (2) -> C
        (3) -> D
        (4) -> E
        (5) -> F
        (6) -> G
        (7) -> H
        (8) -> I
        (9) -> J
        (10) -> K
        (11) -> L
    }
}
