// Pulled from the pahomqtt exampels:
// https://github.com/eclipse/paho.mqtt.rust/blob/master/examples/async_persist_publish.rs
/////////////////////////////////////////////////////////////////////////////

use paho_mqtt as mqtt;
use std::collections::HashMap;
const PERSISTENCE_ERROR: mqtt::Error = mqtt::Error::Paho(mqtt::PERSISTENCE_ERROR);
//use log::trace;

/// The ClientPersistence maps pretty closely to a key/val store. We can use
/// a Rust HashMap to implement an in-memory persistence pretty easily.
/// The keys are strings, and the values are arbitrarily-sized byte buffers.
///
/// Note that this is an extremely silly example, because if you want to use
/// persistence, you probably want it to be out of process so that if the
/// client crashes and restarts, the persistence data still exists.
///
/// This is just here to show how the persistence API callbacks work.
///
#[derive(Default)]
pub struct MemPersistence {
    /// Name derived from the Client ID and Server URI
    /// This could be used to keep a separate persistence store for each
    /// client/server combination.
    name: String,
    /// We'll use a HashMap for a local in-memory store.
    map: HashMap<String, Vec<u8>>,
}

impl MemPersistence {
    /// Create a new/empty persistence store.
    pub fn new() -> Self {
        Self::default()
    }
}

impl mqtt::ClientPersistence for MemPersistence {
    // Open the persistence store.
    // We don't need to do anything here since the store is in memory.
    // We just capture the name for logging/debugging purposes.
    fn open(&mut self, client_id: &str, server_uri: &str) -> mqtt::Result<()> {
        self.name = format!("{}-{}", client_id, server_uri);
        //trace!("Client persistence [{}]: open", self.name);
        Ok(())
    }

    // Close the persistence store.
    // We don't need to do anything.
    fn close(&mut self) -> mqtt::Result<()> {
        //trace!("Client persistence [{}]: close", self.name);
        Ok(())
    }

    // Put data into the persistence store.
    // We get a vector of buffer references for the data to store, which we
    // can concatenate into a single byte buffer to place in the map.
    fn put(&mut self, key: &str, buffers: Vec<&[u8]>) -> mqtt::Result<()> {
        //trace!("Client persistence [{}]: put key '{}'", self.name, key);
        let buf: Vec<u8> = buffers.concat();
        self.map.insert(key.to_string(), buf);
        Ok(())
    }

    // Get (retrieve) data from the persistence store.
    // We look up and return any data corresponding to the specified key.
    fn get(&mut self, key: &str) -> mqtt::Result<Vec<u8>> {
        //trace!("Client persistence [{}]: get key '{}'", self.name, key);
        match self.map.get(key) {
            Some(v) => Ok(v.to_vec()),
            None => Err(PERSISTENCE_ERROR),
        }
    }

    // Remove the key entry from the persistence store, if any.
    fn remove(&mut self, key: &str) -> mqtt::Result<()> {
        //trace!("Client persistence [{}]: remove key '{}'", self.name, key);
        match self.map.remove(key) {
            Some(_) => Ok(()),
            None => Err(PERSISTENCE_ERROR),
        }
    }

    // Retrieve the complete set of keys in the persistence store.
    fn keys(&mut self) -> mqtt::Result<Vec<String>> {
        //trace!("Client persistence [{}]: keys", self.name);
        let mut keys: Vec<String> = Vec::new();
        for key in self.map.keys() {
            keys.push(key.to_string());
        }
        if !keys.is_empty() {
            //trace!("Found keys: {:?}", keys);
        }
        Ok(keys)
    }

    // Clears all the data from the persistence store.
    fn clear(&mut self) -> mqtt::Result<()> {
        //trace!("Client persistence [{}]: clear", self.name);
        self.map.clear();
        Ok(())
    }

    // Determine if the persistence store contains the specified key.
    fn contains_key(&mut self, key: &str) -> bool {
        //trace!("Client persistence [{}]: contains key '{}'", self.name, key);
        self.map.contains_key(key)
    }
}
