use evmap_derive::ShallowCopy;
use std::sync::Arc;

#[derive(ShallowCopy)]
enum Message {
	Quit,
	ChangeColor(i32, i32, i32),
	Move { x: i32, y: i32 },
	Write(String)
}

struct Shallow;

impl evmap::ShallowCopy for Shallow {
	unsafe fn shallow_copy(&self) ->  std::mem::ManuallyDrop<Self> {
		unimplemented!();
	}
}

#[derive(ShallowCopy)]
struct Test {
	f1: i32,
	f2: (i32, i32),
	f3: String,
	f4: Arc<String>,
	f5: Shallow,
	f6: evmap::shallow_copy::CopyValue<i32>,
}
