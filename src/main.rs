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
        "id": "rustminer",
        "method": method,
        "params": params
    });

    let resp: Value = agent.post(RPC_HOST)
        .set("Authorization", &format!("Basic {}", auth))
        .set("Content-Type", "application/json")
        .send_json(payload)?
        .into_json()?;
    
    if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
        return Err(format!("RPC Error: {}", err).into());
    }
    
    Ok(resp["result"].clone())
}

fn sha256d(data: &[u8]) -> [u8; 32] {
    let first = Sha256::digest(data);
    let second = Sha256::digest(&first);
    let mut out = [0u8; 32];
    out.copy_from_slice(&second);
    out
}

fn create_coinbase_tx(height: i64, value: u64) -> Vec<u8> {
    let mut tx = Vec::new();
    tx.write_u32::<LittleEndian>(1).unwrap();
    tx.push(1); // 1 input
    tx.extend_from_slice(&[0u8; 32]); // null prevout
    tx.extend_from_slice(&[0xff; 4]); // index 0xFFFFFFFF

    let h_bytes = (height as u32).to_le_bytes();
    let script_sig = vec![0x01, h_bytes[0], 0x08, 0x00]; 
    tx.push(script_sig.len() as u8);
    tx.extend_from_slice(&script_sig);

    tx.extend_from_slice(&[0xff; 4]); // sequence

    tx.push(1); // 1 output
    tx.write_u64::<LittleEndian>(value).unwrap(); // coinbase reward

    tx.push(1); // script len
    tx.push(0x51); // OP_TRUE (anyone can spend in regtest)

    tx.write_u32::<LittleEndian>(0).unwrap(); // locktime
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
    println!("Starting Safe Rust Multithreaded Miner...");
    
    let agent: ureq::Agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(5))
        .build();
    
    loop {
        let template = match rpc_call(&agent, "getblocktemplate", json!([{"rules": ["segwit"]}])) {
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

        // Generate Coinbase Transaction & calculate correct Merkle Root
        let coinbase_tx = create_coinbase_tx(height, coinbase_value);
        let merkle_root = sha256d(&coinbase_tx);

        let target = bits_to_target(bits_hex);
        
        let mut base_header = [0u8; 76];
        let mut offset = 0;
        
        base_header[offset..offset+4].copy_from_slice(&version.to_le_bytes()); offset += 4;
        base_header[offset..offset+32].copy_from_slice(&prev_hash); offset += 32;
        base_header[offset..offset+32].copy_from_slice(&merkle_root); offset += 32;
        base_header[offset..offset+4].copy_from_slice(&curtime.to_le_bytes()); offset += 4;
        
        let bits_val = u32::from_str_radix(bits_hex, 16).unwrap_or(0x207fffff);
        base_header[offset..offset+4].copy_from_slice(&bits_val.to_le_bytes());

        println!("Mining Block Height {} | Target Bits: {}", height, bits_hex);
        
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
            
            // Construct complete raw block payload: Header (80B) + Tx Count (1) + Coinbase Tx
            let mut full_block_bytes = Vec::new();
            full_block_bytes.extend_from_slice(&final_header);
            full_block_bytes.push(1); // 1 transaction
            full_block_bytes.extend_from_slice(&coinbase_tx);
            
            let block_hex = hex::encode(full_block_bytes);
            
            match rpc_call(&agent, "submitblock", json!([block_hex])) {
                Ok(res) => println!("Block accepted! Response: {:?}", res),
                Err(e) => println!("Submission result: {}", e),
            }
        } else {
            println!("Iteration complete. Sweeping next nonce range...");
        }
    }
                }
