use byteorder::{LittleEndian, WriteBytesExt};
use hex;
use rayon::prelude::*;
use scrypt::{scrypt, Params};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use ureq::Agent;

// --- RPC CONFIGURATION ---
const RPC_URL: &str = "http://127.0.0.1:19332";
const RPC_USER: &str = "rtuser";
const RPC_PASS: &str = "rtpass";

// --- HELPER FUNCTIONS ---
fn double_sha256(data: &[u8]) -> [u8; 32] {
    let first = Sha256::digest(data);
    let second = Sha256::digest(&first);
    let mut result = [0u8; 32];
    result.copy_from_slice(&second);
    result
}

fn reverse_bytes(bytes: &[u8]) -> Vec<u8> {
    let mut rev = bytes.to_vec();
    rev.reverse();
    rev
}

fn encode_varint(val: u64) -> Vec<u8> {
    if val < 0xfd {
        vec![val as u8]
    } else if val <= 0xffff {
        let mut w = vec![0xfd];
        w.write_u16::<LittleEndian>(val as u16).unwrap();
        w
    } else if val <= 0xffffffff {
        let mut w = vec![0xfe];
        w.write_u32::<LittleEndian>(val as u32).unwrap();
        w
    } else {
        let mut w = vec![0xff];
        w.write_u64::<LittleEndian>(val as u64).unwrap();
        w
    }
}

fn build_coinbase_script_sig(height: u64) -> Vec<u8> {
    let mut script = Vec::new();
    let mut h_bytes = Vec::new();
    let mut h = height;
    while h > 0 {
        h_bytes.push((h & 0xff) as u8);
        h >>= 8;
    }
    if h_bytes.is_empty() {
        h_bytes.push(0);
    } else if h_bytes.last().unwrap() & 0x80 != 0 {
        h_bytes.push(0x00);
    }
    script.push(h_bytes.len() as u8);
    script.extend_from_slice(&h_bytes);
    
    let tag = b"/Silvercoin-MWEB/";
    script.push(tag.len() as u8);
    script.extend_from_slice(tag);
    script
}

fn compute_merkle_root(mut txids: Vec<[u8; 32]>) -> [u8; 32] {
    if txids.is_empty() {
        return [0u8; 32];
    }
    while txids.len() > 1 {
        if txids.len() % 2 != 0 {
            let last = *txids.last().unwrap();
            txids.push(last);
        }
        let mut next_level = Vec::new();
        for chunk in txids.chunks(2) {
            let mut concat = Vec::with_capacity(64);
            concat.extend_from_slice(&chunk[0]);
            concat.extend_from_slice(&chunk[1]);
            next_level.push(double_sha256(&concat));
        }
        txids = next_level;
    }
    txids[0]
}

// Construct Coinbase Tx with witness commitment support
fn build_coinbase_tx(height: u64, value: u64, witness_commitment: Option<&str>) -> (Vec<u8>, [u8; 32]) {
    let mut tx = Vec::new();
    
    // Version 1
    tx.write_u32::<LittleEndian>(1).unwrap();
    
    // Inputs (1)
    tx.push(1);
    tx.extend_from_slice(&[0u8; 32]); // Prevout hash
    tx.write_u32::<LittleEndian>(0xffffffff).unwrap(); // Prevout index
    
    let script_sig = build_coinbase_script_sig(height);
    tx.extend_from_slice(&encode_varint(script_sig.len() as u64));
    tx.extend_from_slice(&script_sig);
    tx.write_u32::<LittleEndian>(0xffffffff).unwrap(); // Sequence
    
    // Outputs
    let num_outputs = if witness_commitment.is_some() { 2 } else { 1 };
    tx.extend_from_slice(&encode_varint(num_outputs));
    
    // Output 0: Payout to OP_TRUE (easy regtest spending)
    tx.write_u64::<LittleEndian>(value).unwrap();
    let payout_script = vec![0x51]; // OP_TRUE
    tx.extend_from_slice(&encode_varint(payout_script.len() as u64));
    tx.extend_from_slice(&payout_script);
    
    // Output 1: Witness Commitment (if Segwit/MWEB active)
    if let Some(commit_hex) = witness_commitment {
        tx.write_u64::<LittleEndian>(0).unwrap();
        let commit_bytes = hex::decode(commit_hex).expect("Invalid witness commitment hex");
        tx.extend_from_slice(&encode_varint(commit_bytes.len() as u64));
        tx.extend_from_slice(&commit_bytes);
    }
    
    // Locktime
    tx.write_u32::<LittleEndian>(0).unwrap();
    
    let txid = double_sha256(&tx);
    (tx, txid)
}

fn main() {
    let agent = Agent::new();
    let scrypt_params = Params::new(10, 1, 1, 32).expect("Invalid scrypt params");

    println!("Starting Silvercoin MWEB Solo Miner...");

    loop {
        // 1. Fetch template explicitly signaling SegWit and MWEB rules
        let payload = json!({
            "jsonrpc": "1.0",
            "id": "silvercoin_miner",
            "method": "getblocktemplate",
            "params": [{
                "rules": ["segwit", "mweb"]
            }]
        });

        let resp: Value = match agent.post(RPC_URL)
            .set("Authorization", &format!("Basic {}", base64::encode(format!("{}:{}", RPC_USER, RPC_PASS))))
            .send_json(&payload) {
                Ok(r) => r.into_json().unwrap(),
                Err(e) => {
                    eprintln!("RPC Error: {}", e);
                    std::thread::sleep(Duration::from_secs(2));
                    continue;
                }
            };

        if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
            eprintln!("getblocktemplate error: {}", err);
            std::thread::sleep(Duration::from_secs(2));
            continue;
        }

        let template = &resp["result"];
        let height = template["height"].as_u64().unwrap();
        let version = template["version"].as_u64().unwrap() as u32;
        let curtime = template["curtime"].as_u64().unwrap() as u32;
        let bits_hex = template["bits"].as_str().unwrap();
        let bits = u32::from_str_radix(bits_hex, 16).unwrap();
        let coinbase_val = template["coinbasevalue"].as_u64().unwrap();
        
        let prev_block_bytes = reverse_bytes(&hex::decode(template["previousblockhash"].as_str().unwrap()).unwrap());
        let witness_commitment = template.get("default_witness_commitment").and_then(|v| v.as_str());

        // Parse template transactions (includes HogEx when MWEB is active)
        let template_txs = template["transactions"].as_array().unwrap();
        let mut raw_txs = Vec::new();
        let mut txids = Vec::new();

        // 2. Build Coinbase
        let (coinbase_raw, coinbase_txid) = build_coinbase_tx(height, coinbase_val, witness_commitment);
        txids.push(coinbase_txid);
        raw_txs.push(coinbase_raw);

        for tx_val in template_txs {
            let tx_hex = tx_val["data"].as_str().unwrap();
            let raw = hex::decode(tx_hex).unwrap();
            let txid_bytes = reverse_bytes(&hex::decode(tx_val["txid"].as_str().unwrap()).unwrap());
            
            let mut txid_arr = [0u8; 32];
            txid_arr.copy_from_slice(&txid_bytes);
            
            txids.push(txid_arr);
            raw_txs.push(raw);
        }

        let merkle_root = compute_merkle_root(txids);

        // Extract target from bits
        let target = {
            let exponent = (bits >> 24) as usize;
            let mantissa = bits & 0x007fffff;
            let mut t = [0u8; 32];
            if exponent <= 3 {
                let val = mantissa >> (8 * (3 - exponent));
                t[31] = (val & 0xff) as u8;
            } else {
                let idx = 32 - exponent;
                if idx < 32 {
                    t[idx] = ((mantissa >> 16) & 0xff) as u8;
                    t[idx + 1] = ((mantissa >> 8) & 0xff) as u8;
                    t[idx + 2] = (mantissa & 0xff) as u8;
                }
            }
            t
        };

        // 3. Prepare 80-byte block header
        let mut header_base = Vec::with_capacity(80);
        header_base.write_u32::<LittleEndian>(version).unwrap();
        header_base.extend_from_slice(&prev_block_bytes);
        header_base.extend_from_slice(&merkle_root);
        header_base.write_u32::<LittleEndian>(curtime).unwrap();
        header_base.write_u32::<LittleEndian>(bits).unwrap();

        let found = Arc::new(AtomicBool::new(false));
        let solution_nonce = Arc::new(std::sync::atomic::AtomicU32::new(0));

        // 4. Multi-threaded Scrypt PoW Search
        (0..u32::MAX).into_par_iter().step_by(10000).for_each(|base_nonce| {
            if found.load(Ordering::Relaxed) {
                return;
            }

            let mut header = header_base.clone();
            header.resize(80, 0);

            for nonce in base_nonce..base_nonce.saturating_add(10000) {
                let mut current_header = header.clone();
                (&mut current_header[76..80]).write_u32::<LittleEndian>(nonce).unwrap();

                let mut pow_hash = [0u8; 32];
                scrypt(&current_header, &current_header, &scrypt_params, &mut pow_hash).unwrap();

                // Compare Scrypt hash against target (big-endian order)
                let mut hash_rev = pow_hash;
                hash_rev.reverse();

                if hash_rev <= target {
                    found.store(true, Ordering::Relaxed);
                    solution_nonce.store(nonce, Ordering::Relaxed);
                    break;
                }
            }
        });

        if found.load(Ordering::Relaxed) {
            let winning_nonce = solution_nonce.load(Ordering::Relaxed);
            println!("[+] Block Hash Target Hit at Height {}! Nonce: {}", height, winning_nonce);

            let mut final_header = header_base.clone();
            final_header.write_u32::<LittleEndian>(winning_nonce).unwrap();

            // 5. Serialize Full Raw Block (Header + Tx Count + Txs + Optional MWEB Extension)
            let mut block_bytes = Vec::new();
            block_bytes.extend_from_slice(&final_header);
            block_bytes.extend_from_slice(&encode_varint(raw_txs.len() as u64));

            for raw_tx in raw_txs {
                block_bytes.extend_from_slice(&raw_tx);
            }

            // Append raw MWEB byte array extension provided in template
            if let Some(mweb_hex) = template.get("mweb").and_then(|m| m.as_str()) {
                let mweb_bytes = hex::decode(mweb_hex).unwrap_or_default();
                block_bytes.extend_from_slice(&mweb_bytes);
            }

            // 6. Submit Block via RPC
            let submit_payload = json!({
                "jsonrpc": "1.0",
                "id": "silvercoin_miner",
                "method": "submitblock",
                "params": [hex::encode(block_bytes)]
            });

            let submit_resp: Value = agent.post(RPC_URL)
                .set("Authorization", &format!("Basic {}", base64::encode(format!("{}:{}", RPC_USER, RPC_PASS))))
                .send_json(&submit_payload)
                .unwrap()
                .into_json()
                .unwrap();

            if submit_resp["error"].is_null() && submit_resp["result"].is_null() {
                println!("[SUCCESS] Block {} accepted by node!", height);
            } else {
                eprintln!("[ERROR] Block submission rejected: {:?}", submit_resp);
            }
        }
    }
}
