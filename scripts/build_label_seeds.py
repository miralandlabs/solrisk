#!/usr/bin/env python3
"""Regenerate data/*.jsonl and migrations/labels-seed.sql from public sources."""

import json
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

OFAC_SOL = [
    (
        "WSSoJFMBEKBbAMwRqnMfjt1urtsFGBTMqjsqbBpVMpC",
        "ofac",
        "sanctioned_mixer",
        100,
        "Tornado Cash Solana OFAC SDN 2022",
    ),
    (
        "2wJKRVR9qfGEF2u7YLg7CQHZ68BKXL3UnKfMfSZ5Mij",
        "ofac",
        "sanctioned_mixer",
        100,
        "Tornado Cash Solana OFAC SDN 2022",
    ),
    (
        "CdsVyKLyFe1zvyd8boDjzKUC2GjeKLM5SU3M3ZpCHtS",
        "ofac",
        "sanctioned_mixer",
        100,
        "Tornado Cash Solana OFAC SDN",
    ),
    (
        "4PG6e97DLCn2PRN4ZMmTLg83jsetrDkvamr3JiXoiffa",
        "chainabuse",
        "drainer_program",
        90,
        "tokenpredict/defex drainer program",
    ),
    (
        "2XxegSqY92y6mAb7fpJBpArxQyxLYhp2DsrEubjYmfxR",
        "chainabuse",
        "drainer_operator",
        90,
        "tokenpredict/defex operator wallet",
    ),
]

ALLOW = [
    ("JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4", "jupiter", "verified_protocol", -10, "Jupiter v6"),
    ("whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc", "orca", "verified_protocol", -10, "Orca Whirlpool"),
    ("675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8", "raydium", "verified_protocol", -10, "Raydium AMM v4"),
    ("MarBmsSgAUdvfWojF1rQbvGqK5KcT1Gzcpc5XgH9pX2", "marinade", "verified_protocol", -8, "Marinade Finance"),
    ("PhoeNiXZ8ByJGLkxNfZRnkUfjvmuDqkbE5S4bM1Y5Xf", "phoenix", "verified_protocol", -8, "Phoenix DEX"),
    ("LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo", "meteora", "verified_protocol", -8, "Meteora DLMM"),
    ("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P", "pumpfun", "verified_protocol", -5, "Pump.fun"),
    ("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA", "spl", "verified_protocol", -5, "SPL Token"),
    ("11111111111111111111111111111111", "system", "verified_protocol", -5, "System program"),
    ("ComputeBudget111111111111111111111111111111", "system", "verified_protocol", -5, "Compute Budget"),
    ("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL", "spl", "verified_protocol", -5, "ATA program"),
    ("So11111111111111111111111111111111111111112", "spl", "verified_asset", -5, "Wrapped SOL"),
    ("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v", "circle", "verified_asset", -8, "USDC mint"),
    ("Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB", "tether", "verified_asset", -8, "USDT mint"),
    ("7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuQosE6pz", "wormhole", "verified_protocol", -8, "Wormhole bridge"),
    ("worm2ZoG2kUd4vFXhvjh93HHJS3cLdRBwUZtfKzK47R", "wormhole", "verified_protocol", -8, "Wormhole core"),
    ("DjVE6JNiYqPL2QXyCUUh8rNjHrbz9hTjYm2j8J8pJq8K", "jito", "verified_protocol", -8, "Jito"),
    ("DttWaMuVvTiduZRnguLF7jNxTgiMBZ1hyAumKUiL2KRL", "tensor", "verified_protocol", -5, "Tensor"),
    ("TSWAPaqyCSx2KABk68r77tYaa2vKGxuNkZ5ACW8W2k", "tensor", "verified_protocol", -5, "Tensor swap"),
    ("MERLuDFBMmsHnsBPZw2sDQZHvXFMwp8EdjudcU2HKky", "mercurial", "verified_protocol", -5, "Mercurial"),
    ("MangoCzJ36NCz9ajKuYKa4AWWtz3M7o2BAtp4qZJjP3", "mango", "verified_protocol", -5, "Mango v3"),
    ("srmqPvymJeFKQ4zGQed1GFppgkRHL9kaELCbyksJtPX", "serum", "verified_protocol", -5, "OpenBook/Serum"),
]


def main() -> None:
    deny: dict[str, dict] = {}
    for w, s, l, wt, n in OFAC_SOL:
        deny[w] = {"wallet": w, "source": s, "label": l, "weight": wt, "note": n}

    with urllib.request.urlopen("https://allenhark.com/blacklist.jsonl", timeout=60) as r:
        for line in r:
            line = line.decode().strip()
            if not line:
                continue
            addr = json.loads(line).get("addr")
            if addr and addr not in deny:
                deny[addr] = {
                    "wallet": addr,
                    "source": "community",
                    "label": "scam_deployer",
                    "weight": 70,
                    "note": "allenhark blacklist.jsonl",
                }

    data_dir = ROOT / "data"
    data_dir.mkdir(exist_ok=True)
    with (data_dir / "denylist.jsonl").open("w") as f:
        for e in deny.values():
            f.write(json.dumps(e, separators=(",", ":")) + "\n")

    with (data_dir / "allowlist.jsonl").open("w") as f:
        for row in ALLOW:
            e = {
                "wallet": row[0],
                "source": row[1],
                "label": row[2],
                "weight": row[3],
                "note": row[4],
            }
            f.write(json.dumps(e, separators=(",", ":")) + "\n")

    sql = [
        "-- solrisk labels seed (run: python3 scripts/build_label_seeds.py to regenerate)",
        "-- Idempotent; safe after migrations/init.sql",
        "",
    ]
    for e in deny.values():
        sql.append(
            "INSERT INTO solrisk_wallet_labels (wallet_pubkey, source, label, weight) "
            f"VALUES ('{e['wallet']}','{e['source']}','{e['label']}',{e['weight']}) "
            "ON CONFLICT (wallet_pubkey, source, label) DO UPDATE SET weight=EXCLUDED.weight;"
        )
    for row in ALLOW:
        sql.append(
            "INSERT INTO solrisk_wallet_labels (wallet_pubkey, source, label, weight) "
            f"VALUES ('{row[0]}','{row[1]}','{row[2]}',{row[3]}) "
            "ON CONFLICT (wallet_pubkey, source, label) DO UPDATE SET weight=EXCLUDED.weight;"
        )

    with (ROOT / "migrations" / "labels-seed.sql").open("w") as f:
        f.write("\n".join(sql) + "\n")

    print(f"deny={len(deny)} allow={len(ALLOW)}")


if __name__ == "__main__":
    main()
