extern crate serde;
extern crate itertools;
extern crate rand;
extern crate bincode;

mod disk_btree;
mod multi_map;
mod wal_file;

use disk_btree::OnDiskBTree;
use multi_map::MultiMap;
use wal_file::{KeyValuePair, RecordFile};

use std::error::Error;
use itertools::merge;
use serde::{Deserialize, Serialize};

const MAX_MEMORY_ITEMS: usize = 1000;

// specify the types for the keys & values
pub trait KeyType: Eq + Ord + Clone + Serialize + for<'de> Deserialize<'de> {}
pub trait ValueType: Ord + Clone + Serialize + for<'de> Deserialize<'de> {}

// provide generic implementations

impl<T> KeyType for T where T: Eq + Ord + Clone + Serialize + for<'de> Deserialize<'de> {}
impl<T> ValueType for T where T: Ord + Clone + Serialize + for<'de> Deserialize<'de> {}

/// This struct holds all the pieces of the BTree mechanism
pub struct BTree<K: KeyType, V: ValueType> {
    tree_file_path: String,       // the path to the tree file
    key_size: usize,              // the size of the key in bytes
    value_size: usize,            // the size of the value in bytes
    wal_file: RecordFile<K, V>,   // write-ahead log for in-memory items
    mem_tree: MultiMap<K, V>,     // in-memory multimap that gets merged with the on-disk BTree
    tree_file: OnDiskBTree<K, V>, // the file backing the whole thing
}

impl<K: KeyType, V: ValueType> BTree<K, V> {
    pub fn new(
        tree_file_path: &String,
        key_size: usize,
        value_size: usize,
    ) -> Result<BTree<K, V>, Box<dyn Error>> {
        // create our in-memory multimap
        let mut mem_tree = MultiMap::<K, V>::new();

        // construct the path to the WAL file for the in-memory multimap
        let wal_file_path = tree_file_path.to_owned() + ".wal";

        // construct our WAL file
        let mut wal_file = RecordFile::<K, V>::new(&wal_file_path, key_size, value_size)?;

        // if we have a WAL file, replay it into the mem_tree
        if wal_file.is_new()? {
            for kv in &mut wal_file {
                mem_tree.insert(kv.key, kv.value);
            }
        }

        // open the data file
        let tree_file = OnDiskBTree::<K, V>::new(tree_file_path.to_owned(), key_size, value_size)?;

        Ok(BTree {
            tree_file_path: tree_file_path.clone(),
            key_size,
            value_size,
            tree_file,
            wal_file,
            mem_tree,
        })
    }

    /// Inserts a key into the BTree
    pub fn insert(&mut self, key: K, value: V) -> Result<(), Box<dyn Error>> {
        let record = KeyValuePair { key, value };

        self.wal_file.insert_record(&record)?;

        let KeyValuePair { key, value } = record;

        let size = self.mem_tree.insert(key, value);

        if size > MAX_MEMORY_ITEMS {
            self.compact()?;
        }

        Ok(())
    }

    pub fn get(&self, key: &K) -> Option<std::collections::btree_set::Iter<V>> {
        self.mem_tree.get(key).map(|btree| btree)
    }

    /// Merges the records on disk with the records in memory
    fn compact(&mut self) -> Result<(), Box<dyn Error>> {
        // create a new on-disk BTree
        let mut new_tree_file = OnDiskBTree::<K, V>::new(
            self.tree_file_path.to_owned() + ".new",
            self.key_size,
            self.value_size,
        )?;

        // get an iterator for the in-memory items
        let mem_iter = self.mem_tree.into_iter();

        // get an iterator to the on-disk items
        let disk_iter = self.tree_file.into_iter();

        for kv in merge(mem_iter, disk_iter) {
            new_tree_file.insert_record(&kv)?;
        }

        Ok(())
    }
}

#[cfg(test)]
#[allow(unused_must_use)]
mod tests {
    use rand::{thread_rng, Rng};
    use std::collections::BTreeSet;
    use std::fs;
    use std::fs::OpenOptions;
    use rand::distributions::Alphanumeric;
    use BTree;

    pub fn gen_temp_name() -> String {
        let file_name: String = thread_rng().sample_iter(&Alphanumeric).take(10).map(char::from).collect();

        String::from("/tmp/") + &file_name + &String::from(".btr")
    }

    fn remove_files(file_path: String) {
        fs::remove_file(&file_path);
        fs::remove_file(file_path + ".wal");
    }

    #[test]
    fn new_blank_file() {
        let file_path = gen_temp_name();

        let btree = BTree::<u8, u8>::new(&file_path, 1, 1).unwrap();

        // make sure our two files were created
        let btf = OpenOptions::new()
            .read(true)
            .write(false)
            .create(false)
            .open(&file_path)
            .unwrap();
        assert_eq!(btf.metadata().unwrap().len(), 0);

        let wal = OpenOptions::new()
            .read(true)
            .write(false)
            .create(false)
            .open(file_path.to_owned() + ".wal")
            .unwrap();
        assert_eq!(wal.metadata().unwrap().len(), 0);

        // make sure they think they're new too
        assert!(btree.wal_file.is_new().unwrap());
        assert_eq!(btree.wal_file.count().unwrap(), 0);

        assert!(btree.tree_file.is_new().unwrap());
        assert_eq!(btree.tree_file.count().unwrap(), 0);

        remove_files(file_path); // remove files assuming it all went well
    }

    #[test]
    fn new_existing_file() {
        let file_path = gen_temp_name();

        // scoped so it is cleaned up
        {
            BTree::<u8, u8>::new(&file_path, 1, 1).unwrap();
        }

        let btree = BTree::<u8, u8>::new(&file_path, 1, 1).unwrap();

        // check our file lengths from the struct
        assert_eq!(btree.tree_file.count().unwrap(), 0);
        assert_eq!(btree.wal_file.count().unwrap(), 0);

        remove_files(file_path); // remove files assuming it all went well
    }

    #[test]
    fn insert_new_u8() {
        let file_path = gen_temp_name();

        let mut btree = BTree::<u8, u8>::new(&file_path, 1, 1).unwrap();

        btree.insert(2, 3).unwrap(); // insert into a new file

        assert_eq!(btree.wal_file.count().unwrap(), 1);
        assert!(btree.mem_tree.contains_key(&2));

        remove_files(file_path); // remove files assuming it all went well
    }

    #[test]
    fn insert_new_str() {
        let file_path = gen_temp_name();

        let mut btree = BTree::<String, String>::new(&file_path, 15, 15).unwrap();

        // insert into a new file
        btree
            .insert("Hello".to_owned(), "World".to_owned())
            .unwrap();

        assert!(!btree.wal_file.is_new().unwrap());
        assert!(btree.mem_tree.contains_key(&String::from("Hello")));

        remove_files(file_path); // remove files assuming it all went well
    }

    #[test]
    fn get_returns_an_iter() {
        let file_path = gen_temp_name();

        // setup tree
        let mut btree = BTree::<String, String>::new(&file_path, 15, 15).unwrap();

        // expected return set
        let mut expected: BTreeSet<String> = BTreeSet::new();
        expected.insert("World".to_string());

        btree.insert("Hello".to_owned(), "World".to_owned());

        // get the set at the hello key
        let set_at_hello: Vec<String> = btree.get(&"Hello".to_string()).unwrap().cloned().collect();

        assert_eq!(set_at_hello, ["World".to_string()]);

        remove_files(file_path); // remove files assuming it all went well
    }

    #[test]
    fn insert_multiple() {
        let file_path = gen_temp_name();

        let mut btree = BTree::<String, String>::new(&file_path, 15, 15).unwrap();

        // insert into a new file
        btree
            .insert("Hello".to_owned(), "World".to_owned())
            .unwrap();
        assert!(!btree.wal_file.is_new().unwrap());

        btree
            .insert("Hello".to_owned(), "Everyone".to_owned())
            .unwrap();
        assert!(!btree.wal_file.is_new().unwrap());

        remove_files(file_path); // remove files assuming it all went well
    }
}
