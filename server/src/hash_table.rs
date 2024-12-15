use std::{
    collections::LinkedList,
    hash::{BuildHasher, Hash, Hasher, RandomState},
    iter::repeat_with,
    sync::{RwLock, RwLockReadGuard},
};

pub type Bucket<K, V> = LinkedList<Node<K, V>>;

pub struct HashTable<K, V, S = RandomState>
where
    K: Hash + Eq,
    S: BuildHasher,
{
    content: Vec<RwLock<Bucket<K, V>>>,
    state: S,
}

impl<K, V> HashTable<K, V, RandomState>
where
    K: Hash + Eq,
    V: Clone,
{
    pub fn new(size: usize) -> Self {
        Self {
            content: repeat_with(|| RwLock::new(LinkedList::new()))
                .take(size)
                .collect(),
            state: RandomState::new(),
        }
    }

    pub fn insert(&self, key: K, val: V) {
        let h = self.hash(&key);
        let index = self.get_index(h);
        let mut target = self.content[index].write().unwrap();
        let existing = target.iter_mut().find(|n| n.k == key);
        if let Some(existing) = existing {
            existing.v = val;
        } else {
            target.push_front(Node { k: key, v: val });
        }
    }

    pub fn get(&self, key: K) -> Option<V> {
        let h = self.hash(&key);
        let index = self.get_index(h);
        let target = self.content[index].read().unwrap();
        target.iter().find(|n| n.k == key).map(|n| n.v.clone())
    }

    pub fn read_bucket(&self, key: K) -> RwLockReadGuard<'_, Bucket<K, V>> {
        let h = self.hash(&key);
        let index = self.get_index(h);
        self.content[index].read().unwrap()
    }

    pub fn remove(&self, key: K) -> Option<V> {
        let h = self.hash(&key);
        let index = self.get_index(h);
        let mut target = self.content[index].write().unwrap();
        let item = target.iter().enumerate().find(|(_, n)| n.k == key)?;
        let split_index = item.0;
        let mut tail = target.split_off(split_index);
        let value = tail.pop_front().expect("list should have item");
        target.append(&mut tail);
        Some(value.v)
    }

    fn hash(&self, key: &K) -> u64 {
        let mut hasher = self.state.build_hasher();
        key.hash(&mut hasher);
        hasher.finish()
    }

    fn get_index(&self, hash: u64) -> usize {
        (hash % self.content.len() as u64) as usize
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Node<K, V> {
    pub k: K,
    pub v: V,
}

#[cfg(test)]
mod test {
    use super::HashTable;

    #[test]
    fn basic() {
        let ht = HashTable::new(100);
        ht.insert(1, "hello");
        ht.insert(8, "world");

        assert_eq!(ht.get(1), Some("hello"));
        assert_eq!(ht.get(8), Some("world"));

        ht.remove(8);
        assert_eq!(ht.remove(1), Some("hello"));
    }
}
