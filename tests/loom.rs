#[cfg(loom)]
#[cfg(test)]
mod loom_tests {
    extern crate evmap;

    use loom::sync::atomic::AtomicUsize;
    use loom::thread;

    use std::sync::atomic::Ordering::SeqCst;
    use std::sync::Arc;

    #[test]
    fn evmap_read_while_remove() {
        loom::model(|| {
            let (r, mut w) = evmap::new();
            w.insert(1, 2);
            w.refresh();

            let val = Arc::new(AtomicUsize::new(0));
            let val_copy = Arc::clone(&val);

            let read_thread = thread::spawn(move || {
                val.store(*r.get_one(&1).as_deref().unwrap(), SeqCst);
            });

            let write_thread = thread::spawn(move || {
                w.remove_entry(1);
            });

            read_thread.join().unwrap();
            write_thread.join().unwrap();

            assert_eq!(val_copy.load(SeqCst), 2);
        });
    }
}
