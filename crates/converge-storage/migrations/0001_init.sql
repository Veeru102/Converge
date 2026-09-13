CREATE TABLE IF NOT EXISTS documents (
    id            TEXT PRIMARY KEY,
    title         TEXT NOT NULL DEFAULT '',
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    head_seq      BIGINT NOT NULL DEFAULT 0,
    snapshot_seq  BIGINT NOT NULL DEFAULT 0
);

-- Accepted ops in sequence order. (doc_id, seq) is the identity; a replica's
-- (replica_id, counter) may legitimately appear twice after a worker restart
-- (see docs/IMPLEMENTATION_RISKS.md P3), so it is only indexed.
CREATE TABLE IF NOT EXISTS ops (
    doc_id        TEXT NOT NULL REFERENCES documents(id),
    seq           BIGINT NOT NULL,
    replica_id    BIGINT NOT NULL,
    counter       BIGINT NOT NULL,
    hlc_wall      BIGINT NOT NULL,
    hlc_logical   INTEGER NOT NULL,
    payload       BYTEA NOT NULL,
    committed_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (doc_id, seq)
);
CREATE INDEX IF NOT EXISTS ops_by_replica ON ops (doc_id, replica_id, counter);

CREATE TABLE IF NOT EXISTS snapshots (
    doc_id      TEXT NOT NULL REFERENCES documents(id),
    seq         BIGINT NOT NULL,
    payload     BYTEA NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (doc_id, seq)
);
