use evmap_derive::ShallowCopy;

#[derive(ShallowCopy)]
struct Broken {
	f1: i32,
	f2: std::cell::Cell<()>,
}

#[derive(ShallowCopy)]
struct AlsoBroken(i32, std::cell::Cell<()>);

fn main() {}
