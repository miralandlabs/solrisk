#!/usr/bin/env bash
# Register solrisk x402 resource manifests with the pr402 facilitator.

set -euo pipefail

CLUSTER="devnet"
WALLET="${X402_MERCHANT_WALLET:-}"
KEYPAIR="${MERCHANT_KEYPAIR:-}"
FACILITATOR=""
NO_PROBE=0
NO_LISTING=0
DRY_RUN=0

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SOLRISK_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
X402_ROOT="$(cd "${SOLRISK_ROOT}/.." && pwd)"
ENROLL="${X402_ROOT}/tools/enroll.mjs"

usage() {
    cat <<'USAGE'
Usage:
  scripts/enroll-resources.sh --cluster devnet --keypair /path/to/merchant-keypair.json
  scripts/enroll-resources.sh --cluster mainnet --wallet <merchant-pubkey> --keypair /path/to/merchant-keypair.json

Options:
  --cluster devnet|mainnet  Select x402 resource manifest and facilitator default.
  --wallet PUBKEY           Merchant wallet pubkey. Defaults to X402_MERCHANT_WALLET or manifest merchantWallet.
  --keypair PATH            Merchant Solana keypair JSON. Defaults to MERCHANT_KEYPAIR.
  --facilitator URL         Override facilitator base URL.
  --no-probe                Skip facilitator probe after registration.
  --no-listing              Register without public listing opt-in.
  --dry-run                 Print payloads without registering.

Notes:
  tx-risk is a reserved route (501, not billed) and is deliberately absent
  from both manifests — do not enroll it as a paid resource.
USAGE
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --cluster) CLUSTER="$2"; shift 2 ;;
        --cluster=*) CLUSTER="${1#*=}"; shift ;;
        --wallet) WALLET="$2"; shift 2 ;;
        --wallet=*) WALLET="${1#*=}"; shift ;;
        --keypair) KEYPAIR="$2"; shift 2 ;;
        --keypair=*) KEYPAIR="${1#*=}"; shift ;;
        --facilitator) FACILITATOR="$2"; shift 2 ;;
        --facilitator=*) FACILITATOR="${1#*=}"; shift ;;
        --no-probe) NO_PROBE=1; shift ;;
        --no-listing) NO_LISTING=1; shift ;;
        --dry-run) DRY_RUN=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) echo "unknown arg: $1" >&2; usage >&2; exit 64 ;;
    esac
done

case "$CLUSTER" in
    devnet)
        MANIFEST="${SCRIPT_DIR}/x402-resources-devnet.json"
        FACILITATOR="${FACILITATOR:-https://preview.ipay.sh/api/v1/facilitator}"
        ;;
    mainnet)
        MANIFEST="${SCRIPT_DIR}/x402-resources-mainnet.json"
        FACILITATOR="${FACILITATOR:-https://ipay.sh/api/v1/facilitator}"
        ;;
    *) echo "CLUSTER must be devnet or mainnet" >&2; exit 64 ;;
esac

if [[ ! -f "$ENROLL" ]]; then
    echo "missing parent enrollment tool: ${ENROLL}" >&2
    echo "Run from the x402 monorepo checkout, or copy tools/enroll.mjs into the expected parent tools/ directory." >&2
    exit 66
fi
if [[ -z "$KEYPAIR" ]]; then
    echo "missing --keypair or MERCHANT_KEYPAIR" >&2
    exit 64
fi
if [[ ! -f "$KEYPAIR" ]]; then
    echo "merchant keypair not found: ${KEYPAIR}" >&2
    exit 66
fi

args=(--manifest "$MANIFEST" --keypair "$KEYPAIR" --facilitator "$FACILITATOR")
[[ -n "$WALLET" ]] && args+=(--wallet "$WALLET")
[[ "$NO_PROBE" -eq 1 ]] && args+=(--no-probe)
[[ "$NO_LISTING" -eq 1 ]] && args+=(--no-listing)
[[ "$DRY_RUN" -eq 1 ]] && args+=(--dry-run)

node "$ENROLL" "${args[@]}"
