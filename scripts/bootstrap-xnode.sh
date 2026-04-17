#!/usr/bin/env bash
# Bootstrap a fresh Xnode's domain via the xnode.openmesh.network proxy.
#
# This is a ONE-TIME operation for new xnodes that don't have a domain yet.
# After the domain is set, use `om profile login <name> -u https://<domain>`.
#
# Usage:
#   ./bootstrap-xnode.sh <IP> <SUBDOMAIN> [EMAIL]
#
# Example:
#   ./bootstrap-xnode.sh 74.50.126.86 xnode john@openxai.org

set -euo pipefail

IP="${1:?Usage: $0 <IP> <SUBDOMAIN> [EMAIL]}"
SUBDOMAIN="${2:?Usage: $0 <IP> <SUBDOMAIN> [EMAIL]}"
EMAIL="${3:-john@openxai.org}"
DOMAIN="manager.${SUBDOMAIN}.openmesh.cloud"
PROXY="https://xnode.openmesh.network/api/xnode-forward/${IP}"

echo "=== Bootstrap Xnode Domain ==="
echo "  IP:        ${IP}"
echo "  Domain:    ${DOMAIN}"
echo "  Email:     ${EMAIL}"
echo "  Proxy:     ${PROXY}"
echo ""

# Step 1: Check/claim DNS
echo "1. Checking DNS..."
AVAILABLE=$(curl -s "https://claim.dns.openmesh.network/${SUBDOMAIN}/available")
if [ "$AVAILABLE" = "true" ]; then
    echo "   Claiming ${SUBDOMAIN}..."
    # Need wallet address from om
    WALLET=$(om wallet status 2>&1 | grep "Address:" | awk '{print $NF}' | tr '[:upper:]' '[:lower:]')
    WALLET_ETH="eth:${WALLET#0x}"
    curl -s -X POST "https://claim.dns.openmesh.network/${SUBDOMAIN}/reserve" \
        -H "Content-Type: application/json" \
        -d "{\"user\":\"${WALLET_ETH}\",\"ipv4\":\"${IP}\"}"
    echo "   DNS claimed."
else
    echo "   ${SUBDOMAIN} already claimed."
    RESOLVED=$(dig +short "${DOMAIN}" A 2>/dev/null)
    echo "   ${DOMAIN} → ${RESOLVED}"
fi

# Step 2: Set domain via the frontend proxy
# The proxy at xnode.openmesh.network/api/xnode-forward/ sets Host: manager.xnode.local
echo ""
echo "2. Setting domain on xnode via frontend proxy..."
echo "   This uses the xnode.openmesh.network proxy which sets Host: manager.xnode.local"
echo ""
echo "   NOTE: This step requires you to be logged into xnode.openmesh.network"
echo "   in your browser. The proxy forwards your browser cookies."
echo ""
echo "   Please run this in your browser console instead:"
echo ""
echo "   fetch('/api/xnode-forward/${IP}/os/set', {"
echo "     method: 'POST',"
echo "     headers: { 'Content-Type': 'application/json' },"
echo "     body: JSON.stringify({"
echo "       domain: '${DOMAIN}',"
echo "       acme_email: '${EMAIL}'"
echo "     })"
echo "   }).then(r => r.json()).then(console.log)"
echo ""
echo "   Or use the Subdomain Claimer UI on xnode.openmesh.network"
echo ""
echo "3. After domain is set, run:"
echo "   om profile login v10 -u https://${DOMAIN}"
echo "   om --profile v10 node status"
