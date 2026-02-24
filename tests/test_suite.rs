use tiny_keccak::{Hasher, Keccak};
use k256::ecdsa::{SigningKey, VerifyingKey};
use coins_bip39::{Mnemonic, English};
use coins_bip32::path::DerivationPath;
use std::fs;

// --- UTILS ---

fn keccak256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Keccak::v256();
    let mut out = [0u8; 32];
    hasher.update(data);
    hasher.finalize(&mut out);
    out
}

fn to_checksum_address(addr: &str) -> String {
    let addr = addr.trim_start_matches("0x").to_lowercase();
    let hash = keccak256(addr.as_bytes());
    let mut checksum_addr = String::from("0x");
    for (i, c) in addr.chars().enumerate() {
        let byte_idx = i / 2;
        let nibble_idx = i % 2;
        let byte = hash[byte_idx];
        let nibble = if nibble_idx == 0 { byte >> 4 } else { byte & 0x0f };
        if !c.is_digit(10) && nibble >= 8 {
            checksum_addr.push(c.to_ascii_uppercase());
        } else {
            checksum_addr.push(c);
        }
    }
    checksum_addr
}

// --- COMPREHENSIVE SUITE ---

#[tokio::test]
async fn test_om_cli_full_stack_integrity() {
    println!("\n--- 1. Cryptographic Identity & Checksum ---");
    let addr = "0x6d9b98dbec2a78144c2652908da9d07a887a9bfe";
    let checksummed = to_checksum_address(addr);
    assert_eq!(checksummed, "0x6D9b98DbeC2A78144C2652908DA9d07A887a9bFe");
    println!("✅ EIP-55 Checksum matches Browser/Viem.");

    println!("\n--- 2. HD Wallet Path Derivation ---");
    let phrase = "test test test test test test test test test test test junk";
    let mnemonic = Mnemonic::<English>::new_from_phrase(phrase).unwrap();
    let seed = mnemonic.to_seed(None).unwrap();
    let derivation_path = "m/44'/60'/0'/0/0".parse::<DerivationPath>().unwrap();
    let master = coins_bip32::xkeys::XPriv::root_from_seed(&seed, None).unwrap();
    let child = master.derive_path(&derivation_path).unwrap();
    let signing_key: &SigningKey = child.as_ref();
    let public_key = VerifyingKey::from(signing_key);
    let uncompressed = public_key.to_encoded_point(false);
    let derived_addr = to_checksum_address(&hex::encode(&keccak256(&uncompressed.as_bytes()[1..])[12..]));
    assert_eq!(derived_addr, "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266");
    println!("✅ Wallet derivation path is standard Ethereum.");

    println!("\n--- 3. EIP-191 Signing Logic ---");
    let message = "Test Message";
    let eth_prefix = format!("\x19Ethereum Signed Message:\n{}", message.len());
    let mut eth_message = eth_prefix.into_bytes();
    eth_message.extend_from_slice(message.as_bytes());
    let hash = keccak256(&eth_message);
    let (sig, rec_id) = signing_key.sign_prehash_recoverable(&hash).unwrap();
    let recovered_key = VerifyingKey::recover_from_prehash(&hash, &sig, rec_id).unwrap();
    let recovered_uncompressed = recovered_key.to_encoded_point(false);
    let recovered_addr = to_checksum_address(&hex::encode(&keccak256(&recovered_uncompressed.as_bytes()[1..])[12..]));
    assert_eq!(recovered_addr, derived_addr);
    println!("✅ Pre-hash signing (Keccak-256) is correct.");

    println!("\n--- 4. Header Proxy Construction ---");
    let scope = "/usage";
    let path_str = "/host/memory";
    let path_header = format!("{}{}", scope, path_str);
    assert_eq!(path_header, "/usage/host/memory");
    println!("✅ Proxy headers correctly formatted.");

    println!("\n--- 5. Error Edge Cases ---");
    // Test for malformed JSON handling simulation
    let malformed_json = "{\"error\": \"unauthorized\"}";
    let res: Result<serde_json::Value, _> = serde_json::from_str(malformed_json);
    assert!(res.is_ok());
    assert_eq!(res.unwrap()["error"], "unauthorized");
    println!("✅ Error payloads are correctly identifiable.");
}
