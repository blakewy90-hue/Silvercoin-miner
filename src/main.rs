use std::time::Duration;
use serde_json::{json, Value};
use byteorder::{LittleEndian, WriteBytesExt};
use rayon::prelude::*;
use scrypt::{scrypt, Params};
use hex;
use sha2::{Digest, Sha256};

const RPC_HOST: &str = "http://127.0.0.1:19334";
const RPC_USER: &str = "admin";
const RPC_PASS: &str = "password123";

fn rpc_call(agent: &ureq::Agent, method: &str, params: Value) -> Result<Value, Box<dyn std::error::Error>> {
    let auth = base64::encode(format!("{}:{}", RPC_USER, RPC_PASS));
    let payload = json!({
        "jsonrpc": "1.0",
        "id": "silvercoin_miner",
        "method": method,
        "params": params
    });

    let res = agent.post(RPC_HOST)
        .set("Authorization", &format!("Basic {}", auth))
        .set("Content-Type", "application/json")
        .send_json(payload);

    match res {
        Ok(response) => {
            let resp: Value = response.into_json()?;
            if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
                return Err(format!("RPC Error: {}", err).into());
            }
            Ok(resp["result"].clone())
        }
        Err(ureq::Error::Status(code, response)) => {
            let err_body = response.into_string().unwrap_or_else(|_| "Unknown error".into());
            Err(format!("HTTP {} Error: {}", code, err_body).into())
        }
        Err(e) => Err(e.into()),
    }
}

fn write_varint(vec: &mut Vec<u8>, n: u64) {
    if n < 0xfd {
        vec.push(n as u8);
    } else if n <= 0xffff {
        vec.push(0xfd);
        vec.write_u16::<LittleEndian>(n as u16).unwrap();
    } else if n <= 0xffffffff {
        vec.push(0xfe);
        vec.write_u32::<LittleEndian>(n as u32).unwrap();
    } else {
        vec.push(0xff);
        vec.write_u64::<LittleEndian>(n).unwrap();
    }
}

fn sha256d(data: &[u8]) -> [u8; 32] {
    let first = Sha256::digest(data);
    let second = Sha256::digest(&first);
    let mut out = [0u8; 32];
    out.copy_from_slice(&second);
    out
}

fn calculate_merkle_root(tx_hashes: &[[u8; 32]]) -> [u8; 32] {
    if tx_hashes.is_empty() {
        return [0u8; 32];
    }
    let mut current_level = tx_hashes.to_vec();
    while current_level.len() > 1 {
        if current_level.len() % 2 != 0 {
            let last = *current_level.last().unwrap();
            current_level.push(last);
        }
        let mut next_level = Vec::new();
        for chunk in current_level.chunks(2) {
            let mut concat = Vec::with_capacity(64);
            concat.extend_from_slice(&chunk[0]);
            concat.extend_from_slice(&chunk[1]);
            next_level.push(sha256d(&concat));
        }
        current_level = next_level;
    }
    current_level[0]
}

fn create_coinbase_tx(height: i64, value: u64, witness_commitment: Option<&str>) -> Vec<u8> {
    let mut tx = Vec::new();
    tx.write_u32::<LittleEndian>(1).unwrap(); // Version

    write_varint(&mut tx, 1); // 1 input
    tx.extend_from_slice(&[0u8; 32]); // Null prevout
    tx.write_u32::<LittleEndian>(0xffffffff).unwrap(); // Index

    // BIP34 Height ScriptSig
    let mut height_bytes = Vec::new();
    let mut h = height as u64;
    while h > 0 {
        height_bytes.push((h & 0xff) as u8);
        h >>= 8;
    }
    if height_bytes.is_empty() {
        height_bytes.push(0);
    } else if height_bytes.last().unwrap() & 0x80 != 0 {
        height_bytes.push(0x00);
    }

    let mut script_sig = Vec::new();
    script_sig.push(height_bytes.len() as u8);
    script_sig.extend_from_slice(&height_bytes);
    script_sig.extend_from_slice(b"/Silvercoin-MWEB-Miner/");

    write_varint(&mut tx, script_sig.len() as u64);
    tx.extend_from_slice(&script_sig);
    tx.write_u32::<LittleEndian>(0xffffffff).unwrap(); // Sequence

    let output_count = if witness_commitment.is_some() { 2 } else { 1 };
    write_varint(&mut tx, output_count);

    // Output 0: Reward payout script (OP_TRUE for local regtest spending)
    tx.write_u64::<LittleEndian>(value).unwrap();
    let script_pubkey = vec![0x51]; // OP_TRUE
    write_varint(&mut tx, script_pubkey.len() as u64);
    tx.extend_from_slice(&script_pubkey);

    // Output 1: Witness/MWEB commitment script if specified by template
    if let Some(commit_hex) = witness_commitment {
        tx.write_u64::<LittleEndian>(0).unwrap();
        if let Ok(commit_bytes) = hex::decode(commit_hex) {
            write_varint(&mut tx, commit_bytes.len() as u64);
            tx.extend_from_slice(&commit_bytes);
        } else {
            write_varint(&mut tx, 0);
        }
    }

    tx.write_u32::<LittleEndian>(0).unwrap(); // Locktime
    tx
}

fn bits_to_target(bits_hex: &str) -> [u8; 32] {
    let bits = u32::from_str_radix(bits_hex, 16).unwrap_or(0x207fffff);
    let exponent = (bits >> 24) as usize;
    let mantissa = bits & 0x00FFFFFF;

    let mut target = [0u8; 32];
    if exponent <= 3 {
        let val = mantissa >> (8 * (3 - exponent));
        target[0..4].copy_from_slice(&val.to_le_bytes());
    } else {
        let val = mantissa.to_le_bytes();
        let offset = exponent - 3;
        if offset < 32 {
            target[offset] = val[0];
            if offset + 1 < 32 { target[offset + 1] = val[1]; }
            if offset + 2 < 32 { target[offset + 2] = val[2]; }
        }
    }
    target
}

fn compute_scrypt_and_check(header: &[u8; 80], target: &[u8; 32]) -> bool {
    let mut output = [0u8; 32];
    let params = Params::new(10, 1, 1, Params::RECOMMENDED_LEN).unwrap();
    let _ = scrypt(header, header, &params, &mut output);

    for i in (0..32).rev() {
        if output[i] < target[i] { return true; }
        if output[i] > target[i] { return false; }
    }
    true
}

fn main() {
    println!("Starting Silvercoin MWEB-Aware Parallel Miner...");

    let agent: ureq::Agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(5))
        .build();

    loop {
        // Request template explicitly signaling both Segwit and MWEB soft-fork rules
        let template = match rpc_call(&agent, "getblocktemplate", json!([{"rules": ["segwit", "mweb"]}])) {
            Ok(t) => t,
            Err(e) => {
                println!("Waiting for node... ({})", e);
                std::thread::sleep(Duration::from_secs(2));
                continue;
            }
        };

        let height = template.get("height").and_then(|v| v.as_i64()).unwrap_or(1);
        let version = template.get("version").and_then(|v| v.as_i64()).unwrap_or(1) as u32;
        let bits_hex = template.get("bits").and_then(|v| v.as_str()).unwrap_or("207fffff");
        let curtime = template.get("curtime").and_then(|v| v.as_i64()).unwrap_or(0) as u32;
        let coinbase_value = template.get("coinbasevalue").and_then(|v| v.as_u64()).unwrap_or(5000000000);
        let witness_commitment = template.get("default_witness_commitment").and_then(|v| v.as_str());

        let prev_hash_str = template
            .get("previousblockhash")
            .and_then(|v| v.as_str())
            .unwrap_or("0000000000000000000000000000000000000000000000000000000000000000");

        let mut prev_hash = hex::decode(prev_hash_str).unwrap_or_else(|_| vec![0u8; 32]);
        if prev_hash.len() == 32 {
            prev_hash.reverse();
        } else {
            prev_hash = vec![0u8; 32];
        }

        // Process block template transactions
        let mut template_tx_bytes = Vec::new();
        let mut tx_hashes = Vec::new();

        // Generate Coinbase transaction and prepend its hash
        let coinbase_tx = create_coinbase_tx(height, coinbase_value, witness_commitment);
        let coinbase_txid = sha256d(&coinbase_tx);
        tx_hashes.push(coinbase_txid);

        if let Some(txs) = template.get("transactions").and_then(|v| v.as_array()) {
            for tx in txs {
                if let Some(raw_hex) = tx.get("data").and_then(|v| v.as_str()) {
                    if let Ok(raw_bytes) = hex::decode(raw_hex) {
                        tx_hashes.push(sha256d(&raw_bytes));
                        template_tx_bytes.push(raw_bytes);
                    }
                }
            }
        }

        let merkle_root = calculate_merkle_root(&tx_hashes);
        let target = bits_to_target(bits_hex);

        let mut base_header = [0u8; 76];
        let mut offset = 0;

        base_header[offset..offset+4].copy_from_slice(&version.to_le_bytes()); offset += 4;
        base_header[offset..offset+32].copy_from_slice(&prev_hash); offset += 32;
        base_header[offset..offset+32].copy_from_slice(&merkle_root); offset += 32;
        base_header[offset..offset+4].copy_from_slice(&curtime.to_le_bytes()); offset += 4;

        let bits_val = u32::from_str_radix(bits_hex, 16).unwrap_or(0x207fffff);
        base_header[offset..offset+4].copy_from_slice(&bits_val.to_le_bytes());

        println!("Mining Block Height {} | Target Bits: {} | Txs: {}", height, bits_hex, tx_hashes.len());

        let start_time = std::time::Instant::now();
        let chunk_size = 500_000u32;
        let mut nonce_base = 0u32;
        let mut winning_nonce = None;
        let mut final_header = [0u8; 80];

        while nonce_base < u32::MAX - chunk_size {
            let result = (nonce_base..nonce_base + chunk_size).into_par_iter().find_any(|&nonce| {
                let mut header = [0u8; 80];
                header[..76].copy_from_slice(&base_header);
                let mut nonce_bytes = &mut header[76..];
                let _ = nonce_bytes.write_u32::<LittleEndian>(nonce);

                compute_scrypt_and_check(&header, &target)
            });

            if let Some(nonce) = result {
                winning_nonce = Some(nonce);
                final_header[..76].copy_from_slice(&base_header);
                let mut nb = &mut final_header[76..];
                let _ = nb.write_u32::<LittleEndian>(nonce);
                break;
            }

            nonce_base += chunk_size;
            if start_time.elapsed().as_secs() > 10 { break; }
        }

        if let Some(nonce) = winning_nonce {
            println!("SUCCESS! Target hit with Nonce: {}", nonce);

            // Assemble complete raw block stream
            let mut full_block_bytes = Vec::new();
            full_block_bytes.extend_from_slice(&final_header);

            // Write total transaction count
            write_varint(&mut full_block_bytes, tx_hashes.len() as u64);

            // Write Coinbase transaction
            full_block_bytes.extend_from_slice(&coinbase_tx);

            // Write remaining template transactions
            for tx_raw in template_tx_bytes {
                full_block_bytes.extend_from_slice(&tx_raw);
            }

            // Append raw MWEB block section if supplied by node template
            if let Some(mweb_hex) = template.get("mweb").and_then(|v| v.as_str()) {
                if let Ok(mweb_bytes) = hex::decode(mweb_hex) {
                    full_block_bytes.extend_from_slice(&mweb_bytes);
                }
            }

            let block_hex = hex::encode(full_block_bytes);

            match rpc_call(&agent, "submitblock", json!([block_hex])) {
                Ok(res) => println!("Block accepted! Node response: {:?}", res),
                Err(e) => println!("Submission note: {}", e),
            }
        } else {
            println!("Iteration complete. Refreshing template...");
        }
    }
            }
