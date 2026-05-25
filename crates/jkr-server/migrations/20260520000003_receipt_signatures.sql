-- Persist the secp256k1 ECDSA signature alongside each receipt row so
-- the /api/v1/llm/recent read path can serve signed receipts after a
-- restart, not just unsigned reconstructions of the canonicalized
-- fields. Without these columns the read path was emitting
-- sig_version=0 / empty strings even when push_receipt had signed the
-- in-memory copy.
ALTER TABLE llm_recent
    ADD COLUMN IF NOT EXISTS sig_version  INTEGER NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS signature    TEXT    NOT NULL DEFAULT '',
    ADD COLUMN IF NOT EXISTS signer_pubkey TEXT   NOT NULL DEFAULT '';
