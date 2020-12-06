use left_right::aliasing::DropBehavior;

// NOTE: These types _cannot_ be public, as doing so may cause external implementations to
// implement different behavior for them, which would make transmutes between
// SomeWrapper<Aliased<T, NoDrop>> and SomeWrapper<Aliased<T, DoDrop>> unsound.
#[doc(hidden)]
#[allow(missing_debug_implementations)]
pub struct NoDrop;
pub(crate) struct DoDrop;

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
