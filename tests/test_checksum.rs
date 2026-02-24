use tiny_keccak::{Hasher, Keccak};

fn to_checksum_address(addr: &str) -> String {
    let addr = addr.trim_start_matches("0x").to_lowercase();
    let mut hasher = Keccak::v256();
    let mut hash = [0u8; 32];
    hasher.update(addr.as_bytes());
    hasher.finalize(&mut hash);

    let mut checksum_addr = String::from("0x");
    for (i, c) in addr.chars().enumerate() {
        // Correct EIP-55: numbers are not upper-cased, but they STILL count as positions for the hash.
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

fn main() {
    let input = "6d9b98dbec2a78144c2652908da9d07a887a9bfe";
    let output = to_checksum_address(input);
    println!("Input:  {}", input);
    println!("Output: {}", output);
    println!("Expect: 0x6D9b98DbeC2A78144C2652908DA9d07A887a9bFe");
    
    if output == "0x6D9b98DbeC2A78144C2652908DA9d07A887a9bFe" {
        println!("✅ SUCCESS!");
    } else {
        println!("❌ FAILED!");
    }
}
