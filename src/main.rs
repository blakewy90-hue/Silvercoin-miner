use std::time::{Duration, Instant};

use base64;
use byteorder::{LittleEndian, WriteBytesExt};
use hex;
use rayon::prelude::*;
use scrypt::{scrypt, Params};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const RPC_HOST: &str = "http://127.0.0.1:19334";
const RPC_USER: &str = "admin";
const RPC_PASS: &str = "password123";

const DEFAULT_BITS: u32 = 0x1e0fffff;
const DEFAULT_BITS_HEX: &str = "1e0fffff";

const NONCE_CHUNK_SIZE: u32 = 500_000;
const RPC_TIMEOUT_SECS: u64 = 10;
const TEMPLATE_RETRY_SECS: u64 = 5;
const MAX_ITERATION_SECS: u64 = 10;

fn rpc_call(method: &str, params: Value) -> Result<Value, Box<dyn std::error::Error>> {
    let auth = base64::encode(format!("{}:{}", RPC_USER, RPC_PASS));

    let payload = json!({
        "jsonrpc": "1.0",
        "id": "rustminer",
        "method": method,
        "params": params
    });

    let resp: Value = ureq::post(RPC_HOST)
        .set("Authorization", &format!("Basic {}", auth))
        .set("Content-Type", "application/json")
        .timeout(Duration::from_secs(RPC_TIMEOUT_SECS))
        .send_json(payload)?
        .into_json()?;

    if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
        return Err(format!("RPC Error: {}", err).into());
    }

    Ok(resp["result"].clone())
}

/// SHA256d helper
fn sha256d(data: &[u8]) -> [u8; 32] {
    let first = Sha256::digest(data);
    let second = Sha256::digest(&first);

    let mut out = [0u8; 32];
    out.copy_from_slice(&second);
    out
}

/// Standard merkle root (internal little-endian)
fn merkle_root_standard(tx_hashes_le: &[Vec<u8>]) -> [u8; 32] {
    if tx_hashes_le.is_empty() {
        return [0u8; 32];
    }

    let mut layer: Vec<[u8; 32]> = tx_hashes_le
        .iter()
        .map(|h| {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(h);
            arr
        })
        .collect();

    while layer.len() > 1 {
        let mut next = Vec::new();

        for chunk in layer.chunks(2) {
            let a = chunk[0];
            let b = if chunk.len() == 2 { chunk[1] } else { chunk[0] };

            let mut combined = Vec::with_capacity(64);
            combined.extend_from_slice(&a);
            combined.extend_from_slice(&b);

            next.push(sha256d(&combined));
        }

        layer = next;
    }

    layer[0]
}

/// MWEB merkle root (same logic)
fn merkle_root_mweb(mweb_hashes_le: &[Vec<u8>]) -> [u8; 32] {
    if mweb_hashes_le.is_empty() {
        return [0u8; 32];
    }
    merkle_root_standard(mweb_hashes_le)
}

/// Combined merkle root = SHA256d(standard || mweb)
fn merkle_root_combined(standard: [u8; 32], mweb: [u8; 32]) -> [u8; 32] {
    let mut buf = Vec::with_capacity(64);
    buf.extend_from_slice(&standard);
    buf.extend_from_slice(&mweb);
    sha256d(&buf)
}

/// Convert compact bits to 32-byte target
fn bits_to_target(bits_hex: &str) -> [u8; 32] {
    let bits = u32::from_str_radix(bits_hex, 16).unwrap_or(DEFAULT_BITS);
    let exponent = (bits >> 24) as usize;
    let mantissa = bits & 0x00FF_FFFF;

    let mut target = [0u8; 32];

    if exponent <= 3 {
        let val = mantissa >> (8 * (3 - exponent));
        target[0..4].copy_from_slice(&val.to_le_bytes());
    } else {
        let val = mantissa.to_le_bytes();
        let offset = exponent - 3;
        if offset < 32 {
            target[offset] = val[0];
            if offset + 1 < 32 {
                target[offset + 1] = val[1];
            }
            if offset + 2 < 32 {
                target[offset + 2] = val[2];
            }
        }
    }

    target
}

/// scrypt(header, header) and compare to target
fn compute_scrypt_and_check(header: &[u8; 80], target: &[u8; 32]) -> bool {
    let mut output = [0u8; 32];

    let params = Params::new(10, 1, 1, Params::RECOMMENDED_LEN)
        .expect("Failed to create scrypt params");

    if scrypt(header, header, &params, &mut output).is_err() {
        return false;
    }

    for i in (0..32).rev() {
        if output[i] < target[i] {
            return true;
        }
        if output[i] > target[i] {
            return false;
        }
    }

    true
}

/// Decode previous block hash (RPC big-endian hex) into internal little-endian
fn decode_prev_hash(prev_hash_str: &str) -> [u8; 32] {
    let mut buf = hex::decode(prev_hash_str).unwrap_or_else(|_| vec![0u8; 32]);

    if buf.len() != 32 {
        buf = vec![0u8; 32];
    }

    buf.reverse();

    let mut out = [0u8; 32];
    out.copy_from_slice(&buf);
    out
}

/// Collect tx hashes (coinbase + normal) in internal LE
fn collect_tx_hashes_le(template: &Value) -> Vec<Vec<u8>> {
    let mut hashes = Vec::new();

    // coinbase first
    if let Some(cb_hash_str) = template
        .get("coinbasetxn")
        .and_then(|cb| cb.get("hash"))
        .and_then(|h| h.as_str())
    {
        if let Ok(mut h) = hex::decode(cb_hash_str) {
            h.reverse();
            hashes.push(h);
        }
    }

    // normal txs
    if let Some(txs) = template.get("transactions").and_then(|v| v.as_array()) {
        for tx in txs {
            if let Some(hash_str) = tx.get("hash").and_then(|h| h.as_str()) {
                if let Ok(mut h) = hex::decode(hash_str) {
                    h.reverse();
                    hashes.push(h);
                }
            }
        }
    }

    hashes
}

/// Get MWEB root (if provided) in internal LE
fn get_mweb_root_le(template: &Value) -> [u8; 32] {
    if let Some(mweb_hex) = template.get("mwebroot").and_then(|v| v.as_str()) {
        if let Ok(mut v) = hex::decode(mweb_hex) {
            if v.len() == 32 {
                v.reverse();
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&v);
                return arr;
            }
        }
    }
    [0u8; 32]
}

fn main() {
    println!("Starting Safe Rust Multithreaded Miner (MWEB default)...");

    loop {
        let template = match rpc_call("getblocktemplate", json!([{"rules": ["segwit", "mweb"]}])) {
            Ok(t) => t,
            Err(e) => {
                println!("Waiting for node... ({})", e);
                std::thread::sleep(Duration::from_secs(TEMPLATE_RETRY_SECS));
                continue;
            }
        };

        let height = template.get("height").and_then(Value::as_i64).unwrap_or(0);
        let version = template
            .get("version")
            .and_then(Value::as_i64)
            .unwrap_or(1) as u32;

        let bits_hex = template
            .get("bits")
            .and_then(Value::as_str)
            .unwrap_or(DEFAULT_BITS_HEX);

        let curtime = template
            .get("curtime")
            .and_then(Value::as_i64)
            .unwrap_or(0) as u32;

        let prev_hash_str = template
            .get("previousblockhash")
            .and_then(Value::as_str)
            .unwrap_or("0000000000000000000000000000000000000000000000000000000000000000");

        let prev_hash = decode_prev_hash(prev_hash_str);

        // Build merkle root (standard + MWEB)
        let tx_hashes_le = collect_tx_hashes_le(&template);
        let standard_root = merkle_root_standard(&tx_hashes_le);
        let mweb_root = get_mweb_root_le(&template);
        let merkle_root = merkle_root_combined(standard_root, mweb_root);

        let target = bits_to_target(bits_hex);

        // Build static header (first 76 bytes)
        let mut base_header = [0u8; 76];
        let mut offset = 0;

        base_header[offset..offset + 4].copy_from_slice(&version.to_le_bytes());
        offset += 4;

        base_header[offset..offset + 32].copy_from_slice(&prev_hash);
        offset += 32;

        base_header[offset..offset + 32].copy_from_slice(&merkle_root);
        offset += 32;

        base_header[offset..offset + 4].copy_from_slice(&curtime.to_le_bytes());
        offset += 4;

        let bits_val = u32::from_str_radix(bits_hex, 16).unwrap_or(DEFAULT_BITS);
        base_header[offset..offset + 4].copy_from_slice(&bits_val.to_le_bytes());

        println!(
            "Mining Block Height {} | Target Bits: {} | txs: {}",
            height,
            bits_hex,
            tx_hashes_le.len()
        );

        let start_time = Instant::now();
        let mut nonce_base: u32 = 0;
        let mut winning_nonce: Option<u32> = None;
        let mut final_header = [0u8; 80];

        while nonce_base <= u32::MAX - NONCE_CHUNK_SIZE {
            let result = (nonce_base..nonce_base + NONCE_CHUNK_SIZE)
                .into_par_iter()
                .find_any(|&nonce| {
                    let mut header = [0u8; 80];
                    header[..76].copy_from_slice(&base_header);

                    {
                        let mut nonce_bytes = &mut header[76..];
                        let _ = nonce_bytes.write_u32::<LittleEndian>(nonce);
                    }

                    compute_scrypt_and_check(&header, &target)
                });

            if let Some(nonce) = result {
                winning_nonce = Some(nonce);

                final_header[..76].copy_from_slice(&base_header);
                {
                    let mut nb = &mut final_header[76..];
                    let _ = nb.write_u32::<LittleEndian>(nonce);
                }

                break;
            }

            nonce_base = nonce_base.saturating_add(NONCE_CHUNK_SIZE);

            if start_time.elapsed().as_secs() > MAX_ITERATION_SECS {
                break;
            }
        }

        if let Some(nonce) = winning_nonce {
            println!("SUCCESS! Target hit with Nonce: {}", nonce);
            let header_hex = hex::encode(final_header);

            match rpc_call("submitblock", json!([header_hex])) {
                Ok(res) => println!("Block accepted: {:?}", res),
                Err(e) => println!("Submission note: {}", e),
            }
        } else {
            println!("Iteration complete. Sweeping next nonce range...");
        }
    }
}