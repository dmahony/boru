//! Ad-hoc verification for atomic_write_json.
//! Build: rustc --edition 2021 -L /home/dan/iroh-gossip-chat/target/debug/deps ...
//! Instead, use: cargo build -p iroh-gossip --lib --features net && cargo run --example <name>
//!
//! This script is run from the iroh-gossip-chat workspace as an example.

use std::fs;

use iroh_gossip::chat_core::atomic_write::atomic_write_json;
use n0_error::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct TestData {
    name: String,
    value: u64,
}

fn main() -> Result<()> {
    let tmp = std::env::temp_dir().join("hermes-verify-atomic-write");
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp)?;

    let path = tmp.join("test.json");
    let data = TestData { name: "hello".into(), value: 42 };

    // Test 1: round-trip
    atomic_write_json(&path, &data, "test")?;
    assert!(path.exists());
    let raw = fs::read_to_string(&path).expect("read");
    let decoded: TestData = serde_json::from_str(&raw).expect("parse");
    assert_eq!(decoded, data);

    // Test 2: no left-over .json.tmp
    assert!(!path.with_extension("json.tmp").exists());

    // Test 3: auto-create parent dirs
    let nested = tmp.join("a").join("b").join("c.json");
    atomic_write_json(&nested, &data, "nested")?;
    assert!(nested.exists());

    // Test 4: overwrite
    let v2 = TestData { name: "world".into(), value: 99 };
    atomic_write_json(&path, &v2, "overwrite")?;
    let raw = fs::read_to_string(&path).expect("read");
    let decoded: TestData = serde_json::from_str(&raw).expect("parse");
    assert_eq!(decoded, v2);

    let _ = fs::remove_dir_all(&tmp);
    println!("ALL PASSED");
    Ok(())
}
