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

                                                                                                                                /// Standard merkle root (internal little-endian hashes)
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

                                                                                                                                                                                                                                                                                                                                            /// MWEB merkle root (same tree logic)
                                                                                                                                                                                                                                                                                                                                            fn merkle_root_mweb(mweb_hashes_le: &[Vec<u8>]) -> [u8; 32] {
                                                                                                                                                                                                                                                                                                                                                if mweb_hashes_le.is_empty() {
                                                                                                                                                                                                                                                                                                                                                        return [0u8; 32];
                                                                                                                                                                                                                                                                                                                                                            }
                                                                                                                                                                                                                                                                                                                                                                merkle_root_standard(mweb_hashes_le)
                                                                                                                                                                                                                                                                                                                                                                }

                                                                                                                                                                                                                                                                                                                                                                /// Combined merkle root = SHA256d(standard_root || mweb_root)
                                                                                                                                                                                                                                                                                                                                                                fn merkle_root_combined(standard: [u8;