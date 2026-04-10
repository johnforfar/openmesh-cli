use anyhow::{Result, anyhow};
use clap::{Parser, Subcommand};
use dialoguer::Input;
use keyring::Entry;
use k256::ecdsa::{SigningKey, VerifyingKey};
use coins_bip39::{Mnemonic, English};
use coins_bip32::path::DerivationPath;
use hex;
use std::time::{SystemTime, UNIX_EPOCH};
use tiny_keccak::{Hasher, Keccak};
use std::fs;

use om::cli::cmd::{app, req};
use om::cli::error::report;
use om::cli::output::OutputFormat;
use om::sdk;
use om::sdk::utils::Session;

#[derive(Parser)]
#[command(name = "om")]
#[command(about = "Openmesh CLI: The Sovereign Node Orchestrator", long_about = None)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Target XNODE server URL or IP (e.g., https://manager.yourdomain.com)
    #[arg(short, long, global = true)]
    url: Option<String>,

    /// Output format. Use `json` for scripts and AI agents.
    #[arg(short, long, global = true, value_enum, default_value_t = OutputFormat::Plain)]
    format: OutputFormat,
}

#[derive(Subcommand)]
enum Commands {
    /// Manage your EVM wallet (import / status / clear)
    Wallet {
        #[command(subcommand)]
        action: WalletAction,
    },
    /// Authenticate with the Sovereign Node
    Login,
    /// Node information and monitoring (info / status)
    Node {
        #[command(subcommand)]
        action: NodeAction,
    },
    /// Manage applications on your Xnode (list / deploy / expose / remove)
    App {
        #[command(subcommand)]
        action: app::AppAction,
    },
    /// Inspect or wait on async requests (show / wait)
    Req {
        #[command(subcommand)]
        action: req::ReqAction,
    },
    /// Manage running processes and containers
    Ps,
}

#[derive(Subcommand)]
enum NodeAction {
    /// Get general node information (hostname, state version)
    Info,
    /// Monitor real-time resource usage (CPU, RAM, Disk)
    Status,
}

#[derive(Subcommand)]
enum WalletAction {
    /// Import an EVM private key or 12/24-word pass phrase securely
    Import,
    /// View the active wallet address
    Status,
    /// Clear the stored wallet from the keychain
    Clear,
}

const APP_NAME: &str = "openmesh-cli";
const KEY_ACCOUNT: &str = "default-evm-key";

fn public_key_to_address(verifying_key: &VerifyingKey) -> String {
    let uncompressed_bytes = verifying_key.to_encoded_point(false);
    let public_key_bytes = &uncompressed_bytes.as_bytes()[1..];
    
    let mut hasher = Keccak::v256();
    let mut hash = [0u8; 32];
    hasher.update(public_key_bytes);
    hasher.finalize(&mut hash);
    
    let addr_hex = hex::encode(&hash[12..]);
    to_checksum_address(&addr_hex)
}

fn to_checksum_address(addr: &str) -> String {
    let addr = addr.trim_start_matches("0x").to_lowercase();
    let mut hasher = Keccak::v256();
    let mut hash = [0u8; 32];
    hasher.update(addr.as_bytes());
    hasher.finalize(&mut hash);

    let mut checksum_addr = String::from("0x");
    for (i, c) in addr.chars().enumerate() {
        if c.is_digit(10) {
            checksum_addr.push(c);
        } else {
            let byte_idx = i / 2;
            let nibble_idx = i % 2;
            let byte = hash[byte_idx];
            let nibble = if nibble_idx == 0 { byte >> 4 } else { byte & 0x0f };
            if nibble >= 8 {
                checksum_addr.push(c.to_ascii_uppercase());
            } else {
                checksum_addr.push(c);
            }
        }
    }
    checksum_addr
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let format = cli.format;

    // Dispatch new-style commands (those built on cli::cmd::*) first.
    // These take ownership of their action via destructuring; legacy
    // handlers below borrow cli.command.
    //
    // We do this by destructuring `cli.command` once, then re-binding
    // any non-new-style variant back into `cli` via a fall-through.
    let cli = match cli {
        Cli { command: Commands::App { action }, .. } => {
            if let Err(e) = app::run(action, format).await {
                std::process::exit(report(&e, format));
            }
            return Ok(());
        }
        Cli { command: Commands::Req { action }, .. } => {
            if let Err(e) = req::run(action, format).await {
                std::process::exit(report(&e, format));
            }
            return Ok(());
        }
        other => other,
    };

    match &cli.command {
        Commands::Wallet { action } => match action {
            WalletAction::Import => {
                println!("Import your identity to your local secure keychain.");
                let secret_input: String = Input::new()
                    .with_prompt("Enter your Secret (Pass Phrase or Private Key)")
                    .report(false)
                    .interact_text()?;

                let clean_secret = secret_input.trim();
                let word_count = clean_secret.split_whitespace().count();

                let pk_hex = if word_count >= 12 {
                    let mnemonic = Mnemonic::<English>::new_from_phrase(clean_secret)
                        .map_err(|_| anyhow!("Invalid pass phrase"))?;
                    let seed = mnemonic.to_seed(None).map_err(|_| anyhow!("Seed derivation failed"))?;
                    let derivation_path = "m/44'/60'/0'/0/0".parse::<DerivationPath>()?;
                    let master = coins_bip32::xkeys::XPriv::root_from_seed(&seed, None)?;
                    let child = master.derive_path(&derivation_path)?;
                    let signing_key_ref: &SigningKey = child.as_ref();
                    hex::encode(signing_key_ref.to_bytes())
                } else {
                    let clean_hex = clean_secret.trim_start_matches("0x");
                    if clean_hex.len() != 64 {
                        return Err(anyhow!("Invalid hex length"));
                    }
                    clean_hex.to_string()
                };

                let entry = Entry::new(APP_NAME, KEY_ACCOUNT).map_err(|e| anyhow!("Keychain error: {}", e))?;
                entry.set_password(&pk_hex).map_err(|e| anyhow!("Failed to save to keychain: {}", e))?;

                let bytes = hex::decode(&pk_hex)?;
                let signing_key = SigningKey::from_bytes(bytes.as_slice().into())?;
                let address = public_key_to_address(&VerifyingKey::from(&signing_key));
                println!("✅ Identity imported successfully!");
                println!("   Address: {}", address);
            }
            WalletAction::Status => {
                let entry = Entry::new(APP_NAME, KEY_ACCOUNT).map_err(|e| anyhow!("Keychain error: {}", e))?;
                match entry.get_password() {
                    Ok(pk_hex) => {
                        let bytes = hex::decode(&pk_hex)?;
                        let signing_key = SigningKey::from_bytes(bytes.as_slice().into())?;
                        let address = public_key_to_address(&VerifyingKey::from(&signing_key));
                        println!("💳 Active Wallet Address: {}", address);
                    }
                    Err(_) => println!("❌ No wallet found. Run 'om wallet import' first."),
                }
            }
            WalletAction::Clear => {
                let entry = Entry::new(APP_NAME, KEY_ACCOUNT).map_err(|e| anyhow!("Keychain error: {}", e))?;
                let _ = entry.delete_password();
                println!("🗑️ Wallet cleared from OS keychain.");
            }
        },
        Commands::Login => {
            let target_url = match &cli.url {
                Some(url) => url.clone(),
                None => {
                    println!("🌐 Please specify the Xnode Manager URL.");
                    Input::<String>::new()
                        .with_prompt("Enter Xnode Manager URL (e.g., https://manager.yourdomain.com)")
                        .interact_text()?
                }
            };
            println!("🔐 Authenticating with Sovereign Node at {}...", target_url);
            
            let entry = Entry::new(APP_NAME, KEY_ACCOUNT).map_err(|e| anyhow!("Keychain error: {}", e))?;
            let pk_hex = entry.get_password().map_err(|_| anyhow!("No wallet found. Run 'om wallet import' first."))?;
            
            let bytes = hex::decode(&pk_hex)?;
            let signing_key = SigningKey::from_bytes(bytes.as_slice().into())?;
            let public_key = VerifyingKey::from(&signing_key);
            let address = public_key_to_address(&public_key);
            println!("   Using Wallet: {}", address);
            
            let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs().to_string();
            
            // Extract domain from URL dynamically
            let url_parsed = url::Url::parse(&target_url).map_err(|e| anyhow!("Invalid URL: {}", e))?;
            let domain = url_parsed.host_str().ok_or_else(|| anyhow!("No host in URL"))?;
            let origin = format!("{}://{}", url_parsed.scheme(), domain);

            // The backend identity in the cookie MUST match the format expected by the manager proxy.
            // Reference code in xnode-address.ts uses .toLowerCase() for the identity string.
            let user_addr_plain = address.trim_start_matches("0x").to_lowercase();
            let user_addr_prefixed = format!("eth:{}", user_addr_plain);
            
            let message = format!("Xnode Auth authenticate {} at {}", domain, timestamp);
            
            let eth_prefix = format!("\x19Ethereum Signed Message:\n{}", message.len());
            let mut eth_message = eth_prefix.into_bytes();
            eth_message.extend_from_slice(message.as_bytes());

            let mut hasher = Keccak::v256();
            let mut hash = [0u8; 32];
            hasher.update(&eth_message);
            hasher.finalize(&mut hash);

            // Use sign_prehash_recoverable to ensure we sign the Keccak-256 hash
            let (signature_bytes, recovery_id) = signing_key.sign_prehash_recoverable(&hash)
                .map_err(|e| anyhow!("Signing failed: {}", e))?;
            
            let mut full_sig = signature_bytes.to_bytes().to_vec();
            // v is 27 or 28 for personal_sign
            full_sig.push(recovery_id.to_byte() + 27); 
            let signature = format!("0x{}", hex::encode(full_sig)); 

            let login_url = format!("{}/xnode-auth/api/login", target_url);
            let login_payload = serde_json::json!({
                "user": user_addr_prefixed,
                "signature": signature,
                "timestamp": timestamp 
            });

            let client = reqwest::Client::builder()
                .danger_accept_invalid_certs(true)
                .build()?;

            let resp = client.post(&login_url)
                .header("Content-Type", "application/json")
                .header("Origin", &origin)
                .header("Host", domain)
                .header("Referer", format!("{}/xnode-auth", origin))
                .header("User-Agent", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/121.0.0.0 Safari/537.36")
                .json(&login_payload)
                .send()
                .await;
            
            let mut session_cookies = Vec::new();
            let mut success = false;

            match resp {
                Ok(r) if r.status().is_success() => {
                    success = true;
                    for cookie in r.headers().get_all("set-cookie") {
                        if let Ok(c) = cookie.to_str() {
                            session_cookies.push(c.to_string());
                        }
                    }
                }
                _ => {
                    println!("⚠️ Rust client failed or returned 400. Attempting fallback via system curl...");
                    let cookie_file = "/tmp/om_cookies.txt";
                    let _ = fs::remove_file(cookie_file); // Ensure fresh cookies
                    let _ = std::process::Command::new("curl")
                        .arg("-s")
                        .arg("-L")
                        .arg("-c").arg(cookie_file)
                        .arg("-X").arg("POST")
                        .arg(&login_url)
                        .arg("-H").arg("Content-Type: application/json")
                        .arg("-H").arg(format!("Origin: {}", origin))
                        .arg("-H").arg(format!("Host: {}", domain))
                        .arg("-H").arg(format!("Referer: {}/xnode-auth", origin))
                        .arg("-A").arg("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/121.0.0.0 Safari/537.36")
                        .arg("-d").arg(serde_json::to_string(&login_payload)?)
                        .output()?;
                    
                    if fs::metadata(cookie_file).is_ok() {
                        let cookie_data = fs::read_to_string(cookie_file)?;
                        session_cookies.clear();
                        for line in cookie_data.lines() {
                            if line.trim().is_empty() || line.starts_with("# ") { continue; }
                            let parts: Vec<&str> = line.split('\t').collect();
                            if parts.len() >= 7 {
                                let name = parts[5];
                                let value = parts[6];
                                session_cookies.push(format!("{}={}", name, value));
                            }
                        }
                        success = session_cookies.iter().any(|c| c.contains("xnode_auth_signature"));
                    }
                }
            }

            if success && !session_cookies.is_empty() {
                println!("✅ Authentication Successful!");
                let session = Session {
                    reqwest_client: client,
                    base_url: target_url,
                    domain: domain.to_string(),
                    cookies: session_cookies,
                };
                session.save().map_err(|e| anyhow!("Failed to save session: {:?}", e))?;
                
                // Copy the robust cookie file to the persistent home directory
                let mut cookie_store_path = Session::get_session_path()?;
                cookie_store_path.set_extension("jar");
                let _ = fs::copy("/tmp/om_cookies.txt", &cookie_store_path);
                
                println!("💾 Session saved to local cache.");
            } else {
                return Err(anyhow!("Authentication failed"));
            }
        },
        Commands::Node { action } => {
            let session = Session::load().map_err(|_| anyhow!("Not logged in. Run 'om login' first."))?;
            match action {
                NodeAction::Info => {
                    println!("📡 Fetching node configuration from {}...", session.base_url);
                    let input = sdk::os::GetInput::new(&session);
                    match sdk::os::get(input).await {
                        Ok(config) => {
                            println!("✅ Node Config Fetched:");
                            println!("   Domain: {}", config.domain.unwrap_or_else(|| "N/A".to_string()));
                            println!("   Owner:  {}", config.xnode_owner.unwrap_or_else(|| "N/A".to_string()));
                            if let Some(email) = config.acme_email {
                                println!("   Email:  {}", email);
                            }
                        }
                        Err(e) => {
                            eprintln!("❌ Error: {:?}", e);
                            println!("   Note: This endpoint might return a different schema on this node version.");
                        }
                    }
                }
                NodeAction::Status => {
                    println!("📊 Real-time Resource Monitor:");
                    let input = sdk::usage::MemoryInput::new_with_path(&session, sdk::usage::MemoryPath { scope: "host".to_string() });
                    match sdk::usage::memory(input).await {
                        Ok(mem) => {
                            let used_gb = mem.used as f64 / 1024.0 / 1024.0 / 1024.0;
                            let total_gb = mem.total as f64 / 1024.0 / 1024.0 / 1024.0;
                            println!("   RAM:    {:.2}GB / {:.2}GB ({:.1}%)", used_gb, total_gb, (mem.used as f64 / mem.total as f64) * 100.0);
                        }
                        Err(e) => eprintln!("❌ Error fetching RAM status: {:?}", e),
                    }

                    let input = sdk::usage::CpuInput::new_with_path(&session, sdk::usage::CpuPath { scope: "host".to_string() });
                    match sdk::usage::cpu(input).await {
                        Ok(cpus) => {
                            if cpus.is_empty() {
                                println!("   CPU:    0% (Idle)");
                            } else {
                                let avg_used: f32 = cpus.iter().map(|c| c.used).sum::<f32>() / cpus.len() as f32;
                                println!("   CPU:    {:.1}% ({} vCPUs)", avg_used, cpus.len());
                            }
                        }
                        Err(e) => eprintln!("❌ Error fetching CPU status: {:?}", e),
                    }
                }
            }
        },
        Commands::Ps => {
            let session = Session::load().map_err(|_| anyhow!("Not logged in. Run 'om login' first."))?;
            println!("📋 Listing processes/containers...");
            let input = sdk::config::ContainersInput::new(&session);
            match sdk::config::containers(input).await {
                Ok(containers) => {
                    println!("✅ Containers found: {}", containers.len());
                    for container in containers {
                        println!("   - {}", container);
                    }
                }
                Err(e) => eprintln!("❌ Error listing containers: {:?}", e),
            }
        }
        // App and Req are handled in the early dispatcher above.
        Commands::App { .. } | Commands::Req { .. } => unreachable!(),
    }

    Ok(())
}
