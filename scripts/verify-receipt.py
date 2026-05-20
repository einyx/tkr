#!/usr/bin/env python3
"""
Verify a tkr-server-emitted receipt signature offline.

Reads a receipt JSON from stdin (or from a file path), reconstructs
the canonical-message bytes the server signed, and verifies the
secp256k1 ECDSA signature against the embedded signer_pubkey. Exits 0
if the signature is valid, 1 otherwise.

This is a reference implementation: the canonical-message format is
documented in docs/operations.md and locked by the
`signer_canonical_message_is_stable` test in
crates/tkr-server/src/main.rs. Re-implement in your language of
choice and the same bytes-in produce the same verdict.

Dependencies (one of):
  pip install cryptography     # preferred — Python's standard binding
  pip install coincurve        # smaller, secp256k1-only

Usage:
  curl https://tkr.prysm.sh/api/v1/llm/recent | jq '.entries[0]' \\
      | python3 scripts/verify-receipt.py

  # Or from a file:
  python3 scripts/verify-receipt.py receipt.json

Exit codes:
  0   signature valid
  1   signature invalid
  2   malformed input / missing fields
  3   crypto library missing (install instructions above)
"""

import json
import sys
from typing import Any


SIG_VERSION_SUPPORTED = 1


def canonical_message(r: dict[str, Any]) -> bytes:
    """Reconstruct the exact UTF-8 bytes the server signed.

    Format pinned by `ReceiptSigner::canonical_message` in
    crates/tkr-server/src/main.rs. Newline-separated, `v1\\n` prefix,
    no trailing newline. Field order is fixed.
    """
    lines = [
        "v1",
        f"ts={r.get('ts', 0)}",
        f"provider={r.get('provider', '')}",
        f"model={r.get('model', '')}",
        f"status={r.get('status', 0)}",
        f"input_tokens={r.get('input_tokens', 0)}",
        f"output_tokens={r.get('output_tokens', 0)}",
        f"duration_ms={r.get('duration_ms', 0)}",
    ]
    return "\n".join(lines).encode("utf-8")


def strip_0x(hex_str: str) -> str:
    return hex_str[2:] if hex_str.startswith("0x") else hex_str


def verify_with_cryptography(
    message: bytes, sig_compact: bytes, pubkey_compressed: bytes
) -> bool:
    """Verify using `cryptography` library. Handles SEC1 compressed
    pubkeys and raw r||s signatures by converting to DER."""
    from cryptography.hazmat.primitives.asymmetric import ec
    from cryptography.hazmat.primitives.asymmetric.utils import (
        encode_dss_signature,
    )
    from cryptography.hazmat.primitives import hashes
    from cryptography.exceptions import InvalidSignature

    if len(sig_compact) != 64:
        raise ValueError(
            f"signature must be 64 bytes compact r||s, got {len(sig_compact)}"
        )
    r = int.from_bytes(sig_compact[:32], "big")
    s = int.from_bytes(sig_compact[32:], "big")
    sig_der = encode_dss_signature(r, s)

    pubkey = ec.EllipticCurvePublicKey.from_encoded_point(
        ec.SECP256K1(), pubkey_compressed
    )
    try:
        pubkey.verify(sig_der, message, ec.ECDSA(hashes.SHA256()))
        return True
    except InvalidSignature:
        return False


def verify_with_coincurve(
    message: bytes, sig_compact: bytes, pubkey_compressed: bytes
) -> bool:
    """Verify using `coincurve` (smaller, secp256k1-only)."""
    import hashlib
    from coincurve import PublicKey

    digest = hashlib.sha256(message).digest()
    pk = PublicKey(pubkey_compressed)
    return pk.verify(sig_compact, digest, hasher=None)


def load_input() -> dict[str, Any]:
    if len(sys.argv) >= 2 and sys.argv[1] not in ("-h", "--help"):
        with open(sys.argv[1]) as f:
            raw = f.read()
    else:
        if sys.stdin.isatty():
            print(__doc__, file=sys.stderr)
            sys.exit(2)
        raw = sys.stdin.read()
    return json.loads(raw)


def main() -> int:
    if "-h" in sys.argv or "--help" in sys.argv:
        print(__doc__)
        return 0

    try:
        receipt = load_input()
    except Exception as e:
        print(f"error: could not parse input JSON: {e}", file=sys.stderr)
        return 2

    sig_version = receipt.get("sig_version")
    if sig_version != SIG_VERSION_SUPPORTED:
        print(
            f"error: unsupported sig_version {sig_version!r} "
            f"(this verifier knows v{SIG_VERSION_SUPPORTED})",
            file=sys.stderr,
        )
        return 2

    for field in ("signature", "signer_pubkey"):
        if not receipt.get(field):
            print(f"error: missing field {field!r}", file=sys.stderr)
            return 2

    try:
        sig_bytes = bytes.fromhex(strip_0x(receipt["signature"]))
        pub_bytes = bytes.fromhex(strip_0x(receipt["signer_pubkey"]))
    except ValueError as e:
        print(f"error: signature/pubkey not valid hex: {e}", file=sys.stderr)
        return 2

    message = canonical_message(receipt)

    # Prefer `cryptography` (more widely installed); fall back to
    # `coincurve` if that's what's available.
    last_err: Exception | None = None
    for fn in (verify_with_cryptography, verify_with_coincurve):
        try:
            ok = fn(message, sig_bytes, pub_bytes)
            break
        except ImportError as e:
            last_err = e
            continue
    else:
        print(
            "error: neither `cryptography` nor `coincurve` is installed.\n"
            "  pip install cryptography     # preferred\n"
            "  pip install coincurve        # smaller alternative\n"
            f"(last import error: {last_err})",
            file=sys.stderr,
        )
        return 3

    if ok:
        print(
            f"✓ valid receipt · {receipt.get('provider', '?')} "
            f"· {receipt.get('model', '?')} "
            f"· ts={receipt.get('ts')} "
            f"· {receipt.get('input_tokens', 0)}/{receipt.get('output_tokens', 0)} tokens "
            f"· signed by {receipt['signer_pubkey'][:14]}…"
        )
        return 0
    else:
        print(
            f"✗ INVALID signature · receipt does not verify against "
            f"signer_pubkey {receipt['signer_pubkey'][:14]}…",
            file=sys.stderr,
        )
        return 1


if __name__ == "__main__":
    sys.exit(main())
