extern crate evmap;

extern crate quickcheck;
#[macro_use(quickcheck)]
extern crate quickcheck_macros;

use std::collections::HashSet;
use std::hash::Hash;

fn set<'a, T: 'a, I>(iter: I) -> HashSet<T>
where
    I: IntoIterator<Item = &'a T>,
    T: Copy + Hash + Eq,
{
    iter.into_iter().cloned().collect()
}

#[quickcheck]
fn contains(insert: Vec<u32>) -> bool {
    let (r, mut w) = evmap::new();
    for &key in &insert {
        w.insert(key, ());
    }
    w.refresh();

    insert.iter().all(|&key| r.get(&key).is_some())
}

#[quickcheck]
fn contains_not(insert: Vec<u8>, not: Vec<u8>) -> bool {
    let (r, mut w) = evmap::new();
    for &key in &insert {
        w.insert(key, ());
    }
    w.refresh();

    let nots = &set(&not) - &set(&insert);
    nots.iter().all(|&key| r.get(&key).is_none())
}

#[quickcheck]
fn insert_remove(insert: Vec<u8>, remove: Vec<u8>) -> bool {
    let (r, mut w) = evmap::new();
    for &key in &insert {
        w.insert(key, ());
    }
    w.refresh();
    for &key in &remove {
        w.remove(key, ());
    }
    w.refresh();
    let elements = &set(&insert) - &set(&remove);
    let mapped: Vec<()> = r.map_into(|_, _| ());
    r.len() == elements.len()
        && mapped.len() == elements.len()
        && elements.iter().all(|k| r.get(k).is_some())
}
