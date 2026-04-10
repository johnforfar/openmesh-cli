//! Tests for the flake editor.
//!
//! These exist because flake_editor manipulates the user's NixOS host
//! configuration. A bug here can corrupt /etc/nixos/flake.nix and break the
//! Xnode rebuild. Every public function in `cli::flake_editor` should have
//! tests covering:
//!   - the happy path
//!   - input validation rejecting unsafe characters (no nix injection)
//!   - the additive guarantee: existing rules for OTHER domains are never
//!     touched
//!   - round-trip: parse(serialize(x)) == x
//!   - graceful error on missing/malformed markers

use om::cli::flake_editor::*;

const SAMPLE_FLAKE: &str = r#"{
  description = "XnodeOS Configuration";
  inputs = { nixpkgs.url = "github:NixOS/nixpkgs"; };
  outputs = { self, nixpkgs, ... }: {
    nixosConfigurations.xnode = nixpkgs.lib.nixosSystem {
      modules = [
        ({ config, ... }: {
# START USER CONFIG

services.xnode-reverse-proxy.rules."blog.example.com" = [ { forward = "http://blog:3000"; } ];

# END USER CONFIG
        })
      ];
    };
  };
}
"#;

const FLAKE_NO_USER_CONFIG: &str = r#"{
  description = "XnodeOS";
  outputs = { self, ... }: { };
}
"#;

const FLAKE_MULTI_RULE: &str = r#"{
# START USER CONFIG

services.xnode-reverse-proxy.rules."a.example.com" = [ { forward = "http://a:1000"; } ];
services.xnode-reverse-proxy.rules."b.example.com" = [ { forward = "http://b:2000"; } ];
services.xnode-reverse-proxy.rules."c.example.com" = [ { forward = "http://c:3000"; } ];

# END USER CONFIG
}
"#;

// =============================================================================
// extract_user_config
// =============================================================================

#[test]
fn extract_user_config_returns_inner_text() {
    let inner = extract_user_config(SAMPLE_FLAKE).unwrap();
    assert!(inner.contains("blog.example.com"));
    assert!(!inner.contains("# START USER CONFIG"));
    assert!(!inner.contains("# END USER CONFIG"));
    assert!(!inner.contains("description"));
}

#[test]
fn extract_user_config_missing_markers_errors() {
    let err = extract_user_config(FLAKE_NO_USER_CONFIG).unwrap_err();
    assert_eq!(err, FlakeEditError::MissingMarkers);
}

// =============================================================================
// replace_user_config — round-trip integrity
// =============================================================================

#[test]
fn replace_user_config_round_trip_identity() {
    let inner = extract_user_config(SAMPLE_FLAKE).unwrap();
    let reconstructed = replace_user_config(SAMPLE_FLAKE, inner).unwrap();
    assert_eq!(reconstructed, SAMPLE_FLAKE);
}

#[test]
fn replace_user_config_preserves_text_outside_markers() {
    let new_inner = "\n# completely different content\n";
    let result = replace_user_config(SAMPLE_FLAKE, new_inner).unwrap();
    // Everything before START USER CONFIG should be byte-identical.
    let before = SAMPLE_FLAKE.split("# START USER CONFIG").next().unwrap();
    assert!(result.starts_with(before));
    // Everything after END USER CONFIG should be byte-identical.
    let after = SAMPLE_FLAKE.split("# END USER CONFIG").nth(1).unwrap();
    assert!(result.ends_with(after));
}

#[test]
fn replace_user_config_missing_markers_errors() {
    let err = replace_user_config(FLAKE_NO_USER_CONFIG, "anything").unwrap_err();
    assert_eq!(err, FlakeEditError::MissingMarkers);
}

// =============================================================================
// parse_exposes
// =============================================================================

#[test]
fn parse_exposes_finds_single_rule() {
    let inner = extract_user_config(SAMPLE_FLAKE).unwrap();
    let exposes = parse_exposes(inner);
    assert_eq!(exposes.len(), 1);
    assert_eq!(exposes[0].domain, "blog.example.com");
    assert_eq!(exposes[0].rules.len(), 1);
    assert_eq!(exposes[0].rules[0].forward, "http://blog:3000");
    assert_eq!(exposes[0].rules[0].path, None);
}

#[test]
fn parse_exposes_finds_multiple_rules() {
    let inner = extract_user_config(FLAKE_MULTI_RULE).unwrap();
    let exposes = parse_exposes(inner);
    assert_eq!(exposes.len(), 3);
    let domains: Vec<&str> = exposes.iter().map(|e| e.domain.as_str()).collect();
    assert_eq!(domains, vec!["a.example.com", "b.example.com", "c.example.com"]);
}

#[test]
fn parse_exposes_handles_path_field() {
    let flake = r#"
# START USER CONFIG
services.xnode-reverse-proxy.rules."api.example.com" = [
  { forward = "http://api:8080"; path = "/v1"; }
  { forward = "http://api:8080"; path = "/v2"; }
];
# END USER CONFIG
"#;
    let inner = extract_user_config(flake).unwrap();
    let exposes = parse_exposes(inner);
    assert_eq!(exposes.len(), 1);
    assert_eq!(exposes[0].rules.len(), 2);
    assert_eq!(exposes[0].rules[0].path.as_deref(), Some("/v1"));
    assert_eq!(exposes[0].rules[1].path.as_deref(), Some("/v2"));
}

#[test]
fn parse_exposes_empty_user_config_returns_empty_vec() {
    let exposes = parse_exposes("");
    assert_eq!(exposes.len(), 0);
}

// =============================================================================
// add_or_replace_expose — happy paths
// =============================================================================

#[test]
fn add_new_expose_to_empty_user_config() {
    let flake = r#"
# START USER CONFIG

# END USER CONFIG
"#;
    let expose = DomainExpose {
        domain: "demo.example.com".into(),
        rules: vec![ProxyRule { forward: "http://demo:8080".into(), path: None }],
    };
    let result = add_or_replace_expose(flake, expose, AddRuleMode::FailIfExists).unwrap();
    assert!(result.contains("demo.example.com"));
    assert!(result.contains("http://demo:8080"));
    // The new rule must live inside the user-config markers.
    let inner = extract_user_config(&result).unwrap();
    assert!(inner.contains("demo.example.com"));
}

#[test]
fn add_new_expose_alongside_existing_preserves_old_rule() {
    let expose = DomainExpose {
        domain: "demo.example.com".into(),
        rules: vec![ProxyRule { forward: "http://demo:8080".into(), path: None }],
    };
    let result = add_or_replace_expose(SAMPLE_FLAKE, expose, AddRuleMode::FailIfExists).unwrap();
    // Both old and new should now be present.
    assert!(result.contains("blog.example.com"), "old rule wiped!");
    assert!(result.contains("http://blog:3000"), "old forward wiped!");
    assert!(result.contains("demo.example.com"), "new rule not added");
    assert!(result.contains("http://demo:8080"), "new forward not added");
}

#[test]
fn add_three_exposes_sequentially_preserves_all() {
    let mut flake = r#"
# START USER CONFIG

# END USER CONFIG
"#
    .to_string();
    for (domain, port) in [
        ("one.example.com", 1111),
        ("two.example.com", 2222),
        ("three.example.com", 3333),
    ] {
        let expose = DomainExpose {
            domain: domain.into(),
            rules: vec![ProxyRule { forward: format!("http://app:{}", port), path: None }],
        };
        flake = add_or_replace_expose(&flake, expose, AddRuleMode::FailIfExists).unwrap();
    }
    let inner = extract_user_config(&flake).unwrap();
    let exposes = parse_exposes(inner);
    assert_eq!(exposes.len(), 3);
    assert!(exposes.iter().any(|e| e.domain == "one.example.com"));
    assert!(exposes.iter().any(|e| e.domain == "two.example.com"));
    assert!(exposes.iter().any(|e| e.domain == "three.example.com"));
}

#[test]
fn add_or_replace_with_fail_if_exists_rejects_duplicate() {
    let expose = DomainExpose {
        domain: "blog.example.com".into(), // already in SAMPLE_FLAKE
        rules: vec![ProxyRule { forward: "http://blog:9999".into(), path: None }],
    };
    let err = add_or_replace_expose(SAMPLE_FLAKE, expose, AddRuleMode::FailIfExists).unwrap_err();
    matches!(err, FlakeEditError::DomainExists(_));
}

#[test]
fn add_or_replace_with_replace_swaps_existing_rule_only() {
    let expose = DomainExpose {
        domain: "b.example.com".into(),
        rules: vec![ProxyRule {
            forward: "http://b-new:9999".into(),
            path: Some("/api".into()),
        }],
    };
    let result = add_or_replace_expose(FLAKE_MULTI_RULE, expose, AddRuleMode::Replace).unwrap();
    let inner = extract_user_config(&result).unwrap();
    let exposes = parse_exposes(inner);
    assert_eq!(exposes.len(), 3);
    let b = exposes.iter().find(|e| e.domain == "b.example.com").unwrap();
    assert_eq!(b.rules[0].forward, "http://b-new:9999");
    assert_eq!(b.rules[0].path.as_deref(), Some("/api"));
    // Other domains untouched.
    let a = exposes.iter().find(|e| e.domain == "a.example.com").unwrap();
    assert_eq!(a.rules[0].forward, "http://a:1000");
    let c = exposes.iter().find(|e| e.domain == "c.example.com").unwrap();
    assert_eq!(c.rules[0].forward, "http://c:3000");
}

// =============================================================================
// remove_expose
// =============================================================================

#[test]
fn remove_expose_drops_only_target_domain() {
    let result = remove_expose(FLAKE_MULTI_RULE, "b.example.com").unwrap();
    let inner = extract_user_config(&result).unwrap();
    let exposes = parse_exposes(inner);
    assert_eq!(exposes.len(), 2);
    assert!(exposes.iter().any(|e| e.domain == "a.example.com"));
    assert!(exposes.iter().any(|e| e.domain == "c.example.com"));
    assert!(!exposes.iter().any(|e| e.domain == "b.example.com"));
}

#[test]
fn remove_expose_unknown_domain_errors() {
    let err = remove_expose(FLAKE_MULTI_RULE, "nonexistent.example.com").unwrap_err();
    matches!(err, FlakeEditError::DomainNotFound(_));
}

// =============================================================================
// Input validation — REJECT NIX INJECTION
// =============================================================================

#[test]
fn add_rejects_domain_with_quote_injection() {
    // An attacker who controls the domain string could try to break out of
    // the nix string literal and inject arbitrary nix code.
    let evil = DomainExpose {
        domain: "evil.com\"; system.activationScripts.x = \"rm -rf /".into(),
        rules: vec![ProxyRule { forward: "http://app:80".into(), path: None }],
    };
    let err = add_or_replace_expose(SAMPLE_FLAKE, evil, AddRuleMode::FailIfExists).unwrap_err();
    matches!(err, FlakeEditError::InvalidInput(_));
}

#[test]
fn add_rejects_domain_with_backtick() {
    let evil = DomainExpose {
        domain: "evil.com`whoami`".into(),
        rules: vec![ProxyRule { forward: "http://app:80".into(), path: None }],
    };
    let err = add_or_replace_expose(SAMPLE_FLAKE, evil, AddRuleMode::FailIfExists).unwrap_err();
    matches!(err, FlakeEditError::InvalidInput(_));
}

#[test]
fn add_rejects_forward_with_quote() {
    let evil = DomainExpose {
        domain: "ok.example.com".into(),
        rules: vec![ProxyRule {
            forward: "http://app:80\"; injected = \"".into(),
            path: None,
        }],
    };
    let err = add_or_replace_expose(SAMPLE_FLAKE, evil, AddRuleMode::FailIfExists).unwrap_err();
    matches!(err, FlakeEditError::InvalidInput(_));
}

#[test]
fn add_rejects_forward_with_newline() {
    let evil = DomainExpose {
        domain: "ok.example.com".into(),
        rules: vec![ProxyRule {
            forward: "http://app:80\nsystem.activationScripts = {};".into(),
            path: None,
        }],
    };
    let err = add_or_replace_expose(SAMPLE_FLAKE, evil, AddRuleMode::FailIfExists).unwrap_err();
    matches!(err, FlakeEditError::InvalidInput(_));
}

#[test]
fn add_rejects_path_without_leading_slash() {
    let bad = DomainExpose {
        domain: "ok.example.com".into(),
        rules: vec![ProxyRule {
            forward: "http://app:80".into(),
            path: Some("api".into()),
        }],
    };
    let err = add_or_replace_expose(SAMPLE_FLAKE, bad, AddRuleMode::FailIfExists).unwrap_err();
    matches!(err, FlakeEditError::InvalidInput(_));
}

#[test]
fn add_rejects_empty_domain() {
    let bad = DomainExpose {
        domain: "".into(),
        rules: vec![ProxyRule { forward: "http://app:80".into(), path: None }],
    };
    let err = add_or_replace_expose(SAMPLE_FLAKE, bad, AddRuleMode::FailIfExists).unwrap_err();
    matches!(err, FlakeEditError::InvalidInput(_));
}

#[test]
fn add_accepts_dotted_subdomain() {
    // The user's existing domain pattern: blog.johnny.openmesh.cloud
    let ok = DomainExpose {
        domain: "demo.johnny.openmesh.cloud".into(),
        rules: vec![ProxyRule { forward: "http://demo:3000".into(), path: None }],
    };
    let result = add_or_replace_expose(SAMPLE_FLAKE, ok, AddRuleMode::FailIfExists);
    assert!(result.is_ok(), "valid 4-part subdomain rejected: {:?}", result);
}

// =============================================================================
// Realistic round-trip with the user's actual flake shape
// =============================================================================

#[test]
fn round_trip_preserves_outer_flake_byte_for_byte() {
    let expose = DomainExpose {
        domain: "demo.example.com".into(),
        rules: vec![ProxyRule { forward: "http://demo:8080".into(), path: None }],
    };
    let result = add_or_replace_expose(SAMPLE_FLAKE, expose, AddRuleMode::FailIfExists).unwrap();

    // The outer brace, description, inputs, and outputs blocks must be
    // identical bytes — only the user-config region may have changed.
    let original_outer_before = SAMPLE_FLAKE.split("# START USER CONFIG").next().unwrap();
    let result_outer_before = result.split("# START USER CONFIG").next().unwrap();
    assert_eq!(original_outer_before, result_outer_before);

    let original_outer_after = SAMPLE_FLAKE.split("# END USER CONFIG").nth(1).unwrap();
    let result_outer_after = result.split("# END USER CONFIG").nth(1).unwrap();
    assert_eq!(original_outer_after, result_outer_after);
}
