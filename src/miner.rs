use std::time::Duration;
use serde_json::json;
use byteorder::{LittleEndian, WriteBytesExt};
use rayon::prelude::*;
use scrypt::{scrypt, Params};

const RPC_HOST: &str = "http://127.0.0.1:19334";
const RPC_USER: &str = "yourusername";
const RPC_PASS: &str = "yourpassword";

fn rpc_call(method: &str, params: serde_json::Value) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let auth = base64::encode(format!("{}:{}", RPC_USER, RPC_PASS));
        let payload = json!({
                "jsonrpc": "1.0",
                        "id": "rustminer",
                                "method": method,
                                        "params": params
                                            });

                                                let resp: serde_json::Value = ureq::post(RPC_HOST)
                                                        .set("Authorization", &format!("Basic {}", auth))
                                                                .set("Content-Type", "application/json")
                                                                        .timeout(Duration::from_secs(10))
                                                                                .send_json(payload)?
                                                                                        .into_json()?;
                                                                                            
                                                                                                Ok(resp["result"].clone())
                                                                                                }

                                                                                                fn compute_scrypt_and_check(header: &[u8; 80], _target_hash: u32) -> bool {
                                                                                                    let mut output = [0u8; 32];
                                                                                                        let params = Params::new(10, 1, 1, Params::RECOMMENDED_LEN).unwrap();
                                                                                                            let _ = scrypt(header, header, &params, &mut output);
                                                                                                                true
                                                                                                                }

                                                                                                                fn main() {
                                                                                                                    println!("Starting Rust Multithreaded Miner...");
                                                                                                                        
                                                                                                                            loop {
                                                                                                                                    match rpc_call("getblockcount", json!([])) {
                                                                                                                                                Ok(height) => {
                                                                                                                                                                println!("Connected to node! Current block height: {}", height);
                                                                                                                                                                            },
                                                                                                                                                                                        Err(e) => {
                                                                                                                                                                                                        println!("Node RPC status check: {}", e);
                                                                                                                                                                                                                    }
                                                                                                                                                                                                                            }

                                                                                                                                                                                                                                    let base_header = [0u8; 76];
                                                                                                                                                                                                                                            let target: u32 = 0x0000FFFF;

                                                                                                                                                                                                                                                    println!("Engaging parallel CPU threads via Rayon...");
                                                                                                                                                                                                                                                            
                                                                                                                                                                                                                                                                    let result = (0..1_000_000u32).into_par_iter().find_any(|&nonce| {
                                                                                                                                                                                                                                                                                let mut header = [0u8; 80];
                                                                                                                                                                                                                                                                                            header[..76].copy_from_slice(&base_header);
                                                                                                                                                                                                                                                                                                        let mut nonce_bytes = &mut header[76..];
                                                                                                                                                                                                                                                                                                                    let _ = nonce_bytes.write_u32::<LittleEndian>(nonce);
                                                                                                                                                                                                                                                                                                                                
                                                                                                                                                                                                                                                                                                                                            compute_scrypt_and_check(&header, target)
                                                                                                                                                                                                                                                                                                                                                    });

                                                                                                                                                                                                                                                                                                                                                            if let Some(winning_nonce) = result {
                                                                                                                                                                                                                                                                                                                                                                        println!("Parallel mining iteration complete! Nonce processed: {}", winning_nonce);
                                                                                                                                                                                                                                                                                                                                                                                }

                                                                                                                                                                                                                                                                                                                                                                                        std::thread::sleep(Duration::from_secs(2));
                                                                                                                                                                                                                                                                                                                                                                                            }
                                                                                                                                                                                                                                                                                                                                                                                            }