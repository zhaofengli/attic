//! Utilities for testing.

pub mod shadow_store;

use tokio::runtime::Runtime;

/// Returns a new Tokio runtime.
pub fn get_runtime() -> Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
}

/// Returns some fake data.
pub fn get_fake_data(len: usize) -> Vec<u8> {
    let mut state = 42u32;
    let mut data = vec![0u8; len];

    for (i, byte) in data.iter_mut().enumerate() {
        (state, _) = state.overflowing_mul(1664525u32);
        (state, _) = state.overflowing_add(1013904223u32);
        *byte = ((state >> (i % 24)) & 0xff) as u8;
    }

    data
}
