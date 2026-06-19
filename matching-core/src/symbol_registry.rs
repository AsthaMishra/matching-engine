use std::{
    collections::HashMap,
    sync::atomic::{AtomicU32, Ordering},
};

static SYMBOL_ID_COUNTER: AtomicU32 = AtomicU32::new(1);

pub struct SymbolRegistry {
    pub registry: HashMap<[u8; 8], u32>,
}

impl Default for SymbolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SymbolRegistry {
    pub fn new() -> Self {
        Self {
            registry: HashMap::new(),
        }
    }

    pub fn look_up(&self, name: [u8; 8]) -> Option<u32> {
        self.registry.get(&name).copied()
    }

    pub fn register(&mut self, name: [u8; 8]) -> u32 {
        let id = next_symbol_id();
        self.registry.insert(name, id);
        id
    }
}

fn next_symbol_id() -> u32 {
    SYMBOL_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
}
